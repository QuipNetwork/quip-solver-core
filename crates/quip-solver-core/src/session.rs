//! Generic gRPC session loop over a UDS bidi stream.
//!
//! Modeled on `quip-mock-miner`: Hello → Welcome → Configure → Ready, job
//! handling with Reject reasons, Status on Ping/Cancel, clean drain on
//! Shutdown / idle timeout. Backends supply only a [`Sampler`].

use crate::cli::CommonArgs;
use crate::display::{energy_units, format_duration_ms};
use crate::job::{
    finalize_result, miner, num_sweeps_from_toml, prepare_job, status_msg, Prepared, SessionTarget,
    TopologyCache, DEFAULT_NUM_SWEEPS,
};
use crate::{CancelToken, Sampler, StreamJob, StreamOutcome, StreamResult};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use quip_proto::v1::miner_service_client::MinerServiceClient;
use quip_proto::v1::{
    coord_msg, miner_msg, Capabilities, CoordMsg, Fatal, JobKind, JobRequest, MinerMsg, Ready,
};
use quip_protocol::session::{
    build_hello, check_welcome, BackendCaps, ExitCode, SessionConfig, SessionError,
};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Write;
use std::process::ExitCode as StdExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::{Endpoint, Uri};

/// Static backend metadata advertised in capabilities and Hello.
#[derive(Clone, Copy, Debug)]
pub struct BackendIdentity {
    /// Backend name advertised in Hello / capabilities (e.g. `"cpu"`, `"cuda"`).
    pub backend: &'static str,
    /// Algorithm name advertised in Hello / capabilities (e.g. `"sa"`, `"gibbs"`).
    pub algorithm: &'static str,
    /// Hard cap on variables accepted for a job.
    pub max_nodes: u32,
    /// Hard cap on edges accepted for a job.
    pub max_edges: u32,
    /// Extra capability names advertised in `Hello` and `Capabilities`
    /// (for example `"streaming"`, `"governor"`). Empty for a plain backend.
    pub features: &'static [&'static str],
    /// Sampling-parameter envelope used by `adapt::adapt_params`.
    pub adapt: crate::adapt::AdaptBounds,
}

/// Failure to open a device / build a sampler, surfaced as `EnvIncompatible`.
#[derive(Debug)]
pub struct OpenError(pub String);

/// A session that ended for a reason with a known exit code, but with no
/// [`SessionError`] to carry it — an unannounced stream close, a transport
/// failure, a coordinator that opened with the wrong message.
///
/// [`map_err_to_exit`] recovers the code by downcasting to this, so these exits
/// stay typed instead of being recognized from the text of a `Display` impl
/// that belongs to another crate.
#[derive(Debug)]
struct SessionExit {
    code: ExitCode,
    detail: String,
}

impl SessionExit {
    fn new(code: ExitCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for SessionExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for SessionExit {}

/// Build the `Fatal` the coordinator gets before the miner disconnects.
///
/// The conformance driver grades on this message, not only on the process exit
/// status: the coordinator has to learn why the session ended from the session,
/// because the miner's exit code is not visible to it.
fn fatal_msg(code: ExitCode, reason: String) -> MinerMsg {
    miner(miner_msg::Msg::Fatal(Fatal {
        #[expect(
            clippy::cast_sign_loss,
            reason = "every ExitCode is a small positive sysexits value; Fatal.exit_code is u32"
        )]
        exit_code: code.as_i32() as u32,
        reason,
        // These are contract/protocol faults, not device faults: restarting the
        // process changes nothing until the configuration or the peer changes.
        restart_required: false,
    }))
}

/// Name of a `CoordMsg` payload, for logs and handshake diagnostics.
fn coord_msg_name(msg: Option<&coord_msg::Msg>) -> &'static str {
    match msg {
        Some(coord_msg::Msg::Welcome(_)) => "Welcome",
        Some(coord_msg::Msg::Configure(_)) => "Configure",
        Some(coord_msg::Msg::Topology(_)) => "Topology",
        Some(coord_msg::Msg::SetTarget(_)) => "SetTarget",
        Some(coord_msg::Msg::Job(_)) => "Job",
        Some(coord_msg::Msg::Cancel(_)) => "Cancel",
        Some(coord_msg::Msg::Ping(_)) => "Ping",
        Some(coord_msg::Msg::GetCapabilities(_)) => "GetCapabilities",
        Some(coord_msg::Msg::Shutdown(_)) => "Shutdown",
        None => "unset",
    }
}

/// Build this solver's [`Capabilities`]. The `--capabilities` flag prints it,
/// and `GetCapabilities` returns it, so both answers come from one place.
#[must_use]
pub fn capabilities(id: &BackendIdentity, stream_width: u32) -> Capabilities {
    Capabilities {
        backend: id.backend.to_owned(),
        algorithm: id.algorithm.to_owned(),
        supported_kinds: vec![JobKind::IsingSample as i32],
        max_nodes: id.max_nodes,
        max_edges: id.max_edges,
        features: id.features.iter().map(|f| (*f).to_owned()).collect(),
        protocol_version: quip_protocol::session::PROTOCOL_VERSION,
        stream_width,
        native_topology_hash: None,
    }
}

/// Protobuf JSON view of [`Capabilities`]. Field order matches the message.
///
/// `native_topology_hash` is omitted when unset, which is the protobuf JSON
/// mapping and keeps the current eight-field output byte-identical. When set,
/// protobuf JSON maps `bytes` to a standard-base64 string, not a number array.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilitiesJson<'a> {
    backend: &'a str,
    algorithm: &'a str,
    supported_kinds: Vec<&'a str>,
    max_nodes: u32,
    max_edges: u32,
    features: &'a [String],
    protocol_version: u32,
    stream_width: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    native_topology_hash: Option<String>,
}

/// The `Capabilities` this solver advertises, from the one place both answers
/// come from.
///
/// SPEC section 8: `--capabilities` and the in-session `Capabilities` reply are
/// the same message, so they must carry the same `stream_width`. The flag runs
/// with the device closed, which rules out `Sampler::stream_width(&self)` and
/// is why the advertised width is [`Sampler::declared_stream_width`] — an
/// answer available without an instance. `--capabilities` used to hardcode `1`
/// here while the session reported the live width, so a multi-lane backend gave
/// two different numbers for one message.
fn advertised_capabilities<S: Sampler>(id: &BackendIdentity) -> Capabilities {
    capabilities(id, S::declared_stream_width())
}

/// The `Capabilities` the in-session reply carries.
///
/// Identical to [`advertised_capabilities`] except for a device-dependent
/// declaration: a backend that declares `0` (width unknown until the device
/// opens) gets the live width filled in, because the session holds the device
/// open. The static `--capabilities` answer keeps the `0`.
fn session_capabilities<S: Sampler>(id: &BackendIdentity, live_width: usize) -> Capabilities {
    let mut caps = advertised_capabilities::<S>(id);
    if caps.stream_width == 0 {
        caps.stream_width = u32::try_from(live_width).unwrap_or(u32::MAX);
    }
    caps
}

/// Render [`Capabilities`] in the protobuf JSON mapping.
///
/// The generated prost types carry no serde derives, and adding them to
/// `quip-proto` would put a serde dependency in the wire crate for one CLI
/// flag. Nine fields is less code than that.
fn capabilities_json_from(c: &Capabilities) -> String {
    let view = CapabilitiesJson {
        backend: &c.backend,
        algorithm: &c.algorithm,
        supported_kinds: c
            .supported_kinds
            .iter()
            .filter_map(|k| JobKind::try_from(*k).ok())
            .map(|k| k.as_str_name())
            .collect(),
        max_nodes: c.max_nodes,
        max_edges: c.max_edges,
        features: &c.features,
        protocol_version: c.protocol_version,
        stream_width: c.stream_width,
        native_topology_hash: c
            .native_topology_hash
            .as_deref()
            .map(|h| STANDARD.encode(h)),
    };
    #[expect(
        clippy::expect_used,
        reason = "CapabilitiesJson is strings and integers; serialization cannot fail"
    )]
    serde_json::to_string(&view).expect("serialize capabilities")
}

fn print_capabilities<S: Sampler>(id: &BackendIdentity) -> ExitCode {
    // The protobuf JSON mapping, so the flag and the session reply agree on
    // field names and on every value. Both answers are built by
    // [`advertised_capabilities`].
    //
    // Routed through `write_and_map` for the same reason `--solve` is:
    // `println!` panics on a closed pipe. Reachability does not depend on the
    // output being large enough to fill the pipe buffer — `--capabilities |
    // false` closes the reader before the write happens, and that panics too.
    let mut line = capabilities_json_from(&advertised_capabilities::<S>(id)).into_bytes();
    line.push(b'\n');
    write_and_map(&mut std::io::stdout(), &line)
}

/// Write `--solve` JSON to `writer` and map the I/O result to an exit code.
///
/// `ErrorKind::BrokenPipe` is not an error: the consumer stopped reading,
/// which is its right. `head` closing the pipe means it got what it asked
/// for. Exiting non-zero would make ordinary shell pipelines report
/// spurious failures. Normal Unix tools die silently on SIGPIPE. Rust
/// suppresses SIGPIPE and surfaces EPIPE instead, and restoring the default
/// handler needs `unsafe`, which this workspace denies.
fn write_and_map<W: Write>(writer: &mut W, bytes: &[u8]) -> ExitCode {
    match writer.write_all(bytes) {
        Ok(()) => ExitCode::Clean,
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::Clean,
        Err(_) => ExitCode::InternalFatal,
    }
}

/// Emit a miner progress line every N completed jobs (v0.2 `mine_work_item`
/// parity).
const PROGRESS_LOG_INTERVAL: u64 = 10;

/// Depth of the read loop's queue to the outbound writer task.
///
/// Carries only control replies — `Ready`, `JobRequest`, `Status`, and
/// prepare-time `Reject`s — which are small and, apart from a burst of rejects
/// from a malformed job run, rare. Sized so the read loop never waits on it in
/// practice while still bounding memory if the peer stops reading entirely.
const CTRL_CHANNEL_DEPTH: usize = 256;

/// How many results the outbound writer handles back-to-back before it serves
/// the control queue.
///
/// The writer prefers results so a busy sampler never backs up, but that
/// preference must not become starvation: a coordinator that sends `Cancel` and
/// waits for the `Status` ack, or `Ping` and waits for the reply, is entitled
/// to an answer while the miner is at full tilt. Eight bounds the wait to a few
/// results while still amortizing the check.
const RESULT_BATCH: u32 = 8;

/// How often to send an HTTP/2 PING on an otherwise quiet session.
///
/// A miner grinding one hard nonce sends nothing for minutes at a time, so the
/// PINGs have to keep going while idle (`keep_alive_while_idle`) or the exact
/// case they exist for is the case they skip.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);

/// How long to wait for a PING ack before treating the coordinator as gone.
const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// How often to re-check whether the sampler worker has finished, while waiting
/// out the shutdown grace window.
const SAMPLER_JOIN_POLL: Duration = Duration::from_millis(20);

/// What the read loop knew about a job at prepare time, held until the writer
/// finalizes it.
///
/// The sampling parameters build the outbound `SamplerMeta`. The rest exists so
/// the completion log can report an attempt the way the v0.2.1 miner did:
/// elapsed wall time, and the requirement the attempt was measured against.
/// Only the read loop sees the session `SetTarget`, so it records the
/// thresholds here rather than sharing the target with the writer.
struct PendingJob {
    num_reads: u32,
    num_sweeps: u32,
    started: std::time::Instant,
    /// Session energy threshold, or `None` when no `SetTarget` has arrived.
    max_energy_milli: Option<i64>,
    min_solutions: u32,
    /// This job's cancellation watermark, as `prepare_job` resolved it. The
    /// writer re-checks it so a `Result` for a generation the coordinator
    /// abandoned cannot reach the wire, whatever the backend decided.
    watermark: Option<u64>,
}

/// `job_id` → its [`PendingJob`], shared between the session's read loop (which
/// inserts) and its outbound writer (which removes).
type PendingParams = Arc<StdMutex<HashMap<Vec<u8>, PendingJob>>>;

/// Render the leading bytes of a job id for logs.
///
/// Job ids are opaque and long. Eight bytes is enough to correlate a completion
/// with its dispatch in a coordinator log without wrapping the line.
fn short_job_id(job_id: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(18);
    for b in job_id.iter().take(8) {
        let _ = write!(s, "{b:02x}");
    }
    if job_id.len() > 8 {
        s.push_str("..");
    }
    s
}

#[expect(
    clippy::cast_precision_loss,
    reason = "job count and elapsed seconds are small; the f64 rate is display-only"
)]
fn log_progress(
    backend: &str,
    jobs_done: u64,
    elapsed: Duration,
    best_energy_milli: i64,
    pending: Option<&PendingJob>,
) {
    let secs = elapsed.as_secs_f64();
    let rate = if secs > 0.0 {
        jobs_done as f64 / secs
    } else {
        0.0
    };
    let best = if best_energy_milli == i64::MAX {
        "n/a".to_owned()
    } else {
        energy_units(best_energy_milli).to_string()
    };
    let (reads, sweeps) = pending.map_or((0, 0), |p| (p.num_reads, p.num_sweeps));
    let requirement = format_requirement(pending);
    tracing::info!(
        "[quip-miner-{backend}] progress: {jobs_done} jobs | {rate:.1} jobs/s | reads={reads} sweeps={sweeps} | best={best} | {requirement}"
    );
}

/// Requirement text shared by the progress line.
///
/// The per-attempt line no longer carries this. Sampling parameters stay
/// constant for a round, so the progress interval is enough.
fn format_requirement(pending: Option<&PendingJob>) -> String {
    match pending.and_then(|p| p.max_energy_milli) {
        Some(max) => format!(
            "requires energy<={}, solutions>={}",
            energy_units(max),
            pending.map_or(0, |p| p.min_solutions)
        ),
        None => "no target set".to_owned(),
    }
}

/// Log one finished attempt, mirroring the v0.2.1 miner's per-attempt line.
///
/// Every terminal outcome is logged, not only the ones that clear the target.
/// A miner that is working correctly but not winning looks identical to a
/// wedged one unless the losing attempts are visible, which is the gap this
/// closes: the only per-job signal before this was [`log_progress`], and it
/// fires once per ten jobs, so a miner taking minutes per job showed nothing
/// for the better part of an hour.
///
/// `pending` is `None` when the writer sees a result for a job the read loop
/// never recorded. That is not reachable through the normal path, so the line
/// still goes out, with the parameters and elapsed time reported as unknown.
fn log_attempt(backend: &str, sr: &StreamResult, pending: Option<&PendingJob>) {
    let job = short_job_id(&sr.job_id);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "attempt wall time in ms fits u64 for any realistic job"
    )]
    let elapsed_ms = pending.map_or(0, |p| p.started.elapsed().as_millis() as u64);
    let device_ms = sr.device_access_time_us / 1000;
    let wall = format_duration_ms(elapsed_ms);
    let device = format_duration_ms(device_ms);

    match &sr.outcome {
        StreamOutcome::Completed(Ok(samples)) => {
            let best = samples.iter().map(|r| r.energy_milli).min();
            // Solutions at or below the session threshold: the count the
            // coordinator scores the attempt on. Without a target none of them
            // qualify yet, so report the raw sample count instead.
            let valid = match pending.and_then(|p| p.max_energy_milli) {
                Some(max) => samples.iter().filter(|r| r.energy_milli <= max).count(),
                None => samples.len(),
            };
            tracing::info!(
                "[quip-miner-{backend}] attempt {job}: energy {}, valid {valid}/{} \
                 | {wall} wall, {device} device",
                best.map_or_else(|| "n/a".to_owned(), |e| energy_units(e).to_string()),
                samples.len(),
            );
        }
        StreamOutcome::Completed(Err(err)) => {
            tracing::warn!("[quip-miner-{backend}] attempt {job}: rejected {err} | {wall} wall",);
        }
        // Expected on every reseed, so this is not a degraded condition.
        StreamOutcome::Cancelled => {
            tracing::debug!("[quip-miner-{backend}] attempt {job}: cancelled after {wall}");
        }
    }
}

/// What the outbound writer shares with the read loop, gathered so the writer
/// takes a handful of arguments instead of a list nobody can read.
struct WriterContext {
    /// Prepare-time parameters per job, removed as each one finalizes.
    pending: PendingParams,
    /// Completed-job counter the read loop publishes in `Status`.
    jobs_done: Arc<AtomicU64>,
    /// Set when the writer sends `Fatal` for an unrecoverable device.
    device_faulted: Arc<AtomicBool>,
    /// Cancellation watermark, re-checked here so no `Result` for an abandoned
    /// generation reaches the wire.
    cancel: CancelToken,
    /// Backend name for the log lines.
    backend: &'static str,
}

/// Send every control reply already queued, without waiting for more.
///
/// Returns `false` when the outbound channel closed and the writer must stop.
/// Clears `ctrl_open` when the read loop is gone, which is what stops the
/// caller's `select!` from spinning on a closed receiver.
async fn drain_queued_ctrl(
    tx: &mpsc::Sender<MinerMsg>,
    ctrl_rx: &mut mpsc::Receiver<MinerMsg>,
    ctrl_open: &mut bool,
) -> bool {
    while *ctrl_open {
        match ctrl_rx.try_recv() {
            Ok(msg) => {
                if tx.send(msg).await.is_err() {
                    return false;
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => *ctrl_open = false,
        }
    }
    true
}

/// Outbound half of a session.
///
/// Reading the inbound stream and writing to the outbound one must not live in
/// the same task. A `tx.send().await` blocks once the outbound channel fills,
/// which happens whenever the coordinator is slow to read — and if that await
/// sits in the same loop as `inbound.message()`, the miner stops reading jobs
/// precisely because it is busy returning results. The coordinator,
/// symmetrically blocked writing jobs, then stops reading results, and both
/// peers park forever. This was reproducible: a 480-credit grant (~89 MB of
/// inline h/J in flight) deadlocked the session within three dispatches.
///
/// So the read loop hands outbound traffic here and never awaits the wire
/// itself. Results (large) arrive on `res_rx` straight from the sampler;
/// control replies (small, rare) arrive on `ctrl_rx` from the read loop.
///
/// After Hello, this task is the only sender on the outbound channel.
/// `run_session` keeps its own `Sender` so the stream stays open until the
/// shutdown path drops it. Returning here after `Fatal` therefore does not
/// close the stream; `run_session` watches this task's `JoinHandle`.
///
/// Returns once both inputs are finished, after sending `Fatal` for a device
/// fault, or as soon as the outbound channel closes.
async fn outbound_writer(
    tx: mpsc::Sender<MinerMsg>,
    mut res_rx: mpsc::Receiver<StreamResult>,
    mut ctrl_rx: mpsc::Receiver<MinerMsg>,
    ctx: WriterContext,
) {
    let WriterContext {
        pending,
        jobs_done,
        device_faulted,
        cancel,
        backend,
    } = ctx;
    // Progress logging (mirrors v0.2 mine_work_item's every-N-attempts line).
    let session_start = std::time::Instant::now();
    let mut best_energy_milli: i64 = i64::MAX;
    let mut done: u64 = 0;
    // Disables the control branch once the read loop is gone. A closed
    // `ctrl_rx` completes `recv()` with `None` immediately and forever, so
    // leaving the branch enabled turns this `select!` into a hot spin for the
    // whole shutdown drain — the stretch between `drop(ctrl_tx)` and the sampler
    // releasing `res_tx`, which lasts as long as the last in-flight job. Results
    // still drain: the branch that stays enabled is the one that waits properly.
    let mut ctrl_open = true;
    // Results handled back-to-back since the control queue was last served.
    let mut result_streak: u32 = 0;
    loop {
        // `biased` alone starves the control branch: a sampler that keeps the
        // result queue non-empty means the first arm is always ready, so Cancel
        // acks, Ping replies, and Capabilities wait for a lull that a busy
        // miner never has. Serve whatever control replies are already queued
        // after every RESULT_BATCH results, which bounds their wait without
        // giving up the result-first ordering that keeps the sampler unblocked.
        if result_streak >= RESULT_BATCH {
            result_streak = 0;
            if !drain_queued_ctrl(&tx, &mut ctrl_rx, &mut ctrl_open).await {
                return;
            }
            // Same end-of-session test as the control branch below: the read
            // loop is gone and the sampler has released its sender.
            if !ctrl_open && res_rx.is_closed() {
                return;
            }
        }
        tokio::select! {
            biased;
            // Drain completed results first so a busy sampler never backs up.
            Some(sr) = res_rx.recv() => {
                result_streak = result_streak.saturating_add(1);
                let entry = {
                    let mut p = match pending.lock() {
                        Ok(p) => p,
                        // Recoverable: the map is a plain HashMap, so a panic
                        // elsewhere cannot have left it half-updated. Logged
                        // because a poisoned lock means some task panicked.
                        Err(poisoned) => {
                            tracing::error!(
                                "pending-job map was poisoned by a panicking task; recovering it"
                            );
                            poisoned.into_inner()
                        }
                    };
                    p.remove(&sr.job_id)
                };
                let (reads, sweeps) = entry
                    .as_ref()
                    .map_or((0, 0), |e| (e.num_reads, e.num_sweeps));
                // Defence in depth for SPEC section 5. `sample_stream` already
                // re-checks the watermark, but a backend that overrides it may
                // only check at dequeue, and a Result for an abandoned
                // generation must not reach the wire. Errors are left alone: a
                // device fault is about the device, not the job.
                let sr = match sr.outcome {
                    StreamOutcome::Completed(Ok(_))
                        if entry.as_ref().is_some_and(|e| cancel.is_cancelled(e.watermark)) =>
                    {
                        tracing::debug!(
                            "[quip-miner-{backend}] dropping a late Result for cancelled job {}",
                            short_job_id(&sr.job_id),
                        );
                        StreamResult { outcome: StreamOutcome::Cancelled, ..sr }
                    }
                    _ => sr,
                };
                // A Cancelled job neither advances progress nor updates
                // best energy; finalize_result just refunds its credit.
                let completed = matches!(sr.outcome, StreamOutcome::Completed(_));
                if let StreamOutcome::Completed(Ok(samples)) = &sr.outcome {
                    if let Some(e) = samples.iter().map(|r| r.energy_milli).min() {
                        best_energy_milli = best_energy_milli.min(e);
                    }
                }
                log_attempt(backend, &sr, entry.as_ref());
                for reply in finalize_result(sr, reads, sweeps, &mut done) {
                    let fatal = matches!(reply.msg, Some(miner_msg::Msg::Fatal(_)));
                    if tx.send(reply).await.is_err() {
                        return;
                    }
                    if fatal {
                        // The device will not recover without a restart. Stop
                        // now rather than keep accepting jobs it cannot serve.
                        device_faulted.store(true, Ordering::Relaxed);
                        return;
                    }
                }
                jobs_done.store(done, Ordering::Relaxed);
                if completed && done > 0 && done.is_multiple_of(PROGRESS_LOG_INTERVAL) {
                    log_progress(
                        backend,
                        done,
                        session_start.elapsed(),
                        best_energy_milli,
                        entry.as_ref(),
                    );
                }
            }
            ctrl = ctrl_rx.recv(), if ctrl_open => {
                result_streak = 0;
                if let Some(msg) = ctrl {
                    if tx.send(msg).await.is_err() {
                        return;
                    }
                } else {
                    // Read loop is gone. Finish if the sampler has already
                    // drained too, otherwise keep draining results with this
                    // branch switched off.
                    ctrl_open = false;
                    if res_rx.is_closed() {
                        return;
                    }
                }
            }
            else => return,
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "single bidi session select-loop; splitting would obscure the control flow"
)]
async fn run_session<S: Sampler>(
    uri: &str,
    miner_id: &str,
    id: &BackendIdentity,
    sampler: Arc<S>,
    sweeps_per_beta: Option<usize>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Resolve token before any network I/O so a missing QUIP_SESSION_TOKEN
    // always maps to exit 77 (never InternalFatal from a connect failure).
    let hello = build_hello(
        miner_id,
        id.backend,
        id.algorithm,
        &[JobKind::IsingSample],
        id.features,
        BackendCaps {
            max_nodes: id.max_nodes,
            max_edges: id.max_edges,
        },
    )?;

    let path = uri.strip_prefix("unix://").unwrap_or(uri).to_string();
    let channel = Endpoint::try_from("http://[::]:50051")? // dummy authority for UDS
        // The keepalive the read loop's comment below assumes. It has to be
        // configured to exist: tonic's defaults leave HTTP/2 PINGs off, so a
        // coordinator that vanished without closing the socket left this session
        // parked in `inbound.message()` indefinitely, with no application-level
        // timeout to catch it by design.
        //
        // Scope, stated so the claim is not overread a second time: PINGs are
        // answered by the peer's HTTP/2 layer, not its application. This detects
        // a peer that is *gone*. It does nothing for a peer that is alive but
        // has stopped reading — that is a flow-control stall, and it needs
        // different tools.
        .http2_keep_alive_interval(KEEPALIVE_INTERVAL)
        .keep_alive_timeout(KEEPALIVE_TIMEOUT)
        .keep_alive_while_idle(true)
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let p = path.clone();
            async move {
                let s = tokio::net::UnixStream::connect(p).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
            }
        }))
        .await?;
    let mut client = MinerServiceClient::new(channel);

    let (tx, rx) = mpsc::channel::<MinerMsg>(16);
    tx.send(miner(miner_msg::Msg::Hello(hello))).await?;

    let mut inbound = client.session(ReceiverStream::new(rx)).await?.into_inner();

    // Streaming sampler on a blocking thread: it pulls StreamJobs and emits
    // StreamResults in completion order, keeping `stream_width` models in flight.
    let width = sampler.stream_width().max(1);
    // The advertised width comes from `declared_stream_width` so `--capabilities`
    // can answer with the device closed. A backend that overrides only the live
    // one advertises a number its sampler contradicts, which is a misdeclaration
    // the operator has to fix in the backend. Declaring 0 is the honest opt-out
    // for a width that is a property of the opened device: no static answer
    // exists, so nothing is misdeclared, and the in-session reply carries the
    // live width instead.
    if S::declared_stream_width() == 0 {
        tracing::debug!(live = width, "device-dependent stream width resolved");
    } else if usize::try_from(S::declared_stream_width()) != Ok(width) {
        tracing::error!(
            declared = S::declared_stream_width(),
            live = width,
            "Sampler::declared_stream_width disagrees with Sampler::stream_width; \
             the advertised Capabilities carry the declared value"
        );
    }
    // Prefetch a full extra batch beyond the active set: the backend keeps each
    // stream lane's NEXT slot pre-loaded so a completed slot rotates into a
    // ready one with no idle gap while it is downloaded and refilled (the
    // 3-slot pipeline in quip-miner-cuda). Without this look-ahead the lane
    // stalls every cycle waiting for the next job to arrive over the wire.
    let prefetch = width.saturating_mul(2);
    let cap = prefetch.max(8);
    let (job_tx, job_rx) = mpsc::channel::<StreamJob>(cap);
    let (res_tx, res_rx) = mpsc::channel::<StreamResult>(cap);
    // Control-plane cancellation watermark: bumped here on `Cancel`, read by the
    // sampler thread to skip/abort jobs from generations the coordinator
    // abandoned on reseed.
    let cancel = CancelToken::default();
    let sampler_thread = {
        let s = Arc::clone(&sampler);
        let cancel = cancel.clone();
        // Named, because this is the thread anyone diagnosing a stalled miner
        // needs to find first. An unnamed thread inherits the process name, and
        // in a live stall this one sat among three other threads all reporting
        // `quip-cuda-sa` in `top -H`, identifiable only by elimination. Held to
        // 15 bytes, which is what Linux stores in `/proc/<pid>/task/*/comm`.
        std::thread::Builder::new()
            .name("quip-sampler".to_owned())
            .spawn(move || s.sample_stream(job_rx, res_tx, cancel))?
    };

    let mut grace_ms: u64 = 5000;
    let mut num_sweeps = DEFAULT_NUM_SWEEPS;
    let mut topology: Option<TopologyCache> = None;
    let mut target: Option<SessionTarget> = None;
    // job_id → (num_reads, num_sweeps) resolved at prepare, for the result meta.
    // Shared: the read loop inserts at prepare, the writer task removes at
    // finalize. The lock is never held across an await.
    let pending: PendingParams = Arc::new(StdMutex::new(HashMap::new()));
    // Published by the writer, read here for `Status.jobs_done`.
    let jobs_done = Arc::new(AtomicU64::new(0));

    let (ctrl_tx, ctrl_rx) = mpsc::channel::<MinerMsg>(CTRL_CHANNEL_DEPTH);
    let device_faulted = Arc::new(AtomicBool::new(false));
    let mut writer = tokio::spawn(outbound_writer(
        tx.clone(),
        res_rx,
        ctrl_rx,
        WriterContext {
            pending: Arc::clone(&pending),
            jobs_done: Arc::clone(&jobs_done),
            device_faulted: Arc::clone(&device_faulted),
            cancel: cancel.clone(),
            backend: id.backend,
        },
    ));
    // Set when the `select!` below polls `writer` to completion. A
    // `JoinHandle` must be joined exactly once: awaiting it again panics.
    let mut writer_joined = false;
    // Why the session ended badly, when it did. Recorded rather than returned
    // on the spot: every early return here would skip the shutdown path below,
    // which is what flushes the outbound half and — the part that is not
    // cosmetic — joins the sampler worker before this function returns.
    let mut writer_failure: Option<String> = None;
    let mut handshake_error: Option<SessionError> = None;
    let mut protocol_failure: Option<SessionExit> = None;
    // `Welcome` is the first message of a session. Until it arrives, nothing
    // has agreed on a protocol version.
    let mut welcome_seen = false;

    loop {
        // Read loop: owns the inbound stream and nothing else. Every outbound
        // message goes through `ctrl_tx` so this loop cannot be blocked by a
        // slow or backed-up peer.
        //
        // No wall-clock idle timeout: a miner grinding a hard nonce for
        // minutes-to-hours legitimately receives nothing from the
        // coordinator meanwhile — it is busy, not dead. Liveness of a
        // truly-gone peer surfaces here as a closed stream (`Ok(None)`) or
        // a transport error (`Err`); dead-peer detection over the network
        // belongs to HTTP/2 keepalive, not an application quiet-period.
        //
        // Also watch the writer. On a device fault it sends `Fatal` and
        // returns while this loop would otherwise sit in `inbound.message()`
        // until the coordinator sent `Shutdown` or closed. `biased` plus
        // writer-first means a completed writer wins when both are ready,
        // so an already-buffered later job is not dispatched after `Fatal`.
        // In the normal path the writer outlives this loop (`res_rx` and
        // `ctrl_rx` stay open until we drop them below), so it cannot win.
        let msg = tokio::select! {
            biased;
            join = &mut writer => {
                writer_joined = true;
                // A panicking writer used to be discarded here, so a miner
                // whose outbound half died mid-session still exited 0 and the
                // supervisor saw a clean stop.
                if let Err(e) = join {
                    writer_failure = Some(join_error_message(e));
                }
                break;
            }
            msg = inbound.message() => msg,
        };
        {
            let cm: CoordMsg = match msg {
                Ok(Some(cm)) => cm,
                Ok(None) => {
                    // A close is only clean when `Shutdown` asked for it, and
                    // that arm breaks the loop itself, so reaching here always
                    // means the coordinator hung up unannounced. Before
                    // `Welcome` that is how a rejected session token looks from
                    // this side: the coordinator reads the `Hello` and drops
                    // the stream instead of answering.
                    protocol_failure = Some(if welcome_seen {
                        SessionExit::new(
                            ExitCode::InternalFatal,
                            "coordinator closed the session stream without Shutdown",
                        )
                    } else {
                        SessionExit::new(
                            ExitCode::TokenRejected,
                            "coordinator closed the session stream before Welcome; \
                             the session token was rejected",
                        )
                    });
                    break;
                }
                Err(status) => {
                    // Typed, rather than left for the message-matching fallback
                    // in `map_err_to_exit`: an authentication failure on the
                    // transport is how a coordinator refuses a token once the
                    // stream is open.
                    let code = match status.code() {
                        tonic::Code::Unauthenticated | tonic::Code::PermissionDenied => {
                            ExitCode::TokenRejected
                        }
                        _ => ExitCode::InternalFatal,
                    };
                    protocol_failure = Some(SessionExit::new(
                        code,
                        format!("session stream failed: {status}"),
                    ));
                    break;
                }
            };
            // Nothing may precede `Welcome`: until it arrives no version has
            // been agreed, so acting on a `Configure` or a `Job` would mean
            // running an unchecked protocol. Refuse it the way a bad `Welcome`
            // is refused.
            if !welcome_seen && !matches!(cm.msg, Some(coord_msg::Msg::Welcome(_)) | None) {
                let detail = format!(
                    "coordinator sent {} before Welcome",
                    coord_msg_name(cm.msg.as_ref())
                );
                if ctrl_tx
                    .send(fatal_msg(ExitCode::ConfigInvalid, detail.clone()))
                    .await
                    .is_err()
                {
                    break;
                }
                protocol_failure = Some(SessionExit::new(ExitCode::ConfigInvalid, detail));
                break;
            }
            match cm.msg {
                Some(coord_msg::Msg::Welcome(w)) => {
                    // The canonical check, so a PROTOCOL_VERSION change cannot
                    // leave a second hand-inlined copy of it behind.
                    if let Err(e) = check_welcome(&w) {
                        // The coordinator learns why the session ended from the
                        // session: it never sees the process exit status.
                        if ctrl_tx
                            .send(fatal_msg(ExitCode::from(e), e.to_string()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                        handshake_error = Some(e);
                        break;
                    }
                    welcome_seen = true;
                }
                Some(coord_msg::Msg::Configure(c)) => {
                    // Hand the verbatim config subsection to the backend to
                    // parse against its own schema (overrides CLI, warns on
                    // unknown fields / overrides) before mining starts.
                    sampler.apply_config(&c.backend_toml);
                    num_sweeps = num_sweeps_from_toml(&c.backend_toml);
                    let config = SessionConfig::from_configure(miner_id.into(), &c);
                    // A closed control channel means the writer is gone; break
                    // so the shutdown path runs, rather than returning past it.
                    if ctrl_tx
                        .send(miner(miner_msg::Msg::Ready(Ready {})))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    // Request enough credits to keep the prefetch buffer
                    // full (active + next per lane), so the backend always
                    // has a NEXT slot to rotate into.
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "stream width / prefetch are small device-local counts well under u32::MAX"
                    )]
                    let depth = config.queue_depth.max(prefetch as u32);
                    // INVARIANT: the granted pool must not exceed the job
                    // channel's capacity (`cap`), or the read loop can block
                    // on `job_tx.send` with jobs still arriving — the same
                    // deadlock one layer down. `cap == prefetch` and
                    // `depth <= prefetch` unless the coordinator's
                    // `queue_depth` is larger, so clamp to `cap`.
                    let depth = depth.min(u32::try_from(cap).unwrap_or(u32::MAX));
                    if ctrl_tx
                        .send(miner(miner_msg::Msg::JobRequest(JobRequest {
                            credits: depth,
                        })))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Some(coord_msg::Msg::Topology(t)) => {
                    topology = Some(TopologyCache::from_proto(&t));
                }
                Some(coord_msg::Msg::SetTarget(s)) => {
                    target = Some(SessionTarget::from_proto(&s));
                }
                Some(coord_msg::Msg::Job(job)) => {
                    match prepare_job(
                        job,
                        &*sampler,
                        id,
                        num_sweeps,
                        sweeps_per_beta,
                        topology.as_ref(),
                        target.as_ref(),
                    ) {
                        Prepared::Reject(msg) => {
                            // Rejecting at prepare time is terminal for the
                            // job, so ask for a replacement credit — same as
                            // a completion — to keep the coordinator's
                            // consume-on-dispatch pool from leaking a slot.
                            if ctrl_tx.send(msg).await.is_err() {
                                break;
                            }
                            if ctrl_tx
                                .send(miner(miner_msg::Msg::JobRequest(JobRequest { credits: 1 })))
                                .await
                                .is_err()
                            {
                                break;
                            }
                        }
                        Prepared::Sample {
                            job,
                            num_reads,
                            num_sweeps: ns,
                        } => {
                            tracing::debug!(
                                "{} received job {}: {} nodes, {} edges | reads={num_reads} sweeps={ns}",
                                id.backend,
                                short_job_id(&job.job_id),
                                job.graph.num_nodes(),
                                job.graph.edges.len(),
                            );
                            {
                                let mut p = match pending.lock() {
                                    Ok(p) => p,
                                    // Recoverable: the map is a plain HashMap,
                                    // so a panic elsewhere cannot have left it
                                    // half-updated. Logged because a poisoned
                                    // lock means some task panicked.
                                    Err(poisoned) => {
                                        tracing::error!(
                                            "pending-job map was poisoned by a panicking task; \
                                             recovering it"
                                        );
                                        poisoned.into_inner()
                                    }
                                };
                                let _ = p.insert(
                                    job.job_id.clone(),
                                    PendingJob {
                                        num_reads,
                                        num_sweeps: ns,
                                        started: std::time::Instant::now(),
                                        watermark: job.watermark,
                                        // `drive` mode sets no real threshold
                                        // and sends `i64::MAX`. Reporting that
                                        // as a requirement is noise.
                                        max_energy_milli: target
                                            .as_ref()
                                            .map(|t| t.max_energy_milli)
                                            .filter(|&m| m != i64::MAX),
                                        min_solutions: target
                                            .as_ref()
                                            .map_or(0, |t| t.min_solutions),
                                    },
                                );
                            }
                            if job_tx.send(job).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                Some(coord_msg::Msg::Cancel(c)) => {
                    // Abandon every generation at/below the watermark; the
                    // sampler skips buffered stale jobs at dequeue and (in
                    // backends that override sample_stream) aborts the
                    // in-flight one at its next checkpoint.
                    cancel.cancel_through(c.max_generation);
                    if ctrl_tx
                        .send(status_msg(
                            miner_id,
                            jobs_done.load(Ordering::Relaxed),
                            sampler.utilization(),
                            cancel.abandoned(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Some(coord_msg::Msg::Ping(_)) => {
                    if ctrl_tx
                        .send(status_msg(
                            miner_id,
                            jobs_done.load(Ordering::Relaxed),
                            sampler.utilization(),
                            cancel.abandoned(),
                        ))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Some(coord_msg::Msg::GetCapabilities(_)) => {
                    // Same message `--capabilities` prints, same values —
                    // except a device-dependent width declaration (0), which
                    // the open device resolves.
                    if ctrl_tx
                        .send(miner(miner_msg::Msg::Capabilities(
                            session_capabilities::<S>(id, width),
                        )))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Some(coord_msg::Msg::Shutdown(s)) => {
                    grace_ms = if s.grace_ms == 0 {
                        5000
                    } else {
                        u64::from(s.grace_ms)
                    };
                    break;
                }
                // An unset oneof: either a coordinator bug or a field number
                // this build does not know, which prost keeps as an unknown
                // field and reports as no payload at all. Dropping it silently
                // made a peer speaking a newer dialect look like a quiet one.
                None => tracing::warn!(
                    "ignoring a CoordMsg with no recognized payload \
                     (unset, or a field this build does not know)"
                ),
            }
        }
    }

    // Stop feeding, then let the writer flush in-flight results within the
    // grace window. Closing both of its inputs is what ends it: `job_tx` stops
    // the sampler, which drops `res_tx`; `ctrl_tx` closes the control side.
    // The writer drains whatever the sampler already produced before returning,
    // so the drain that used to live here now happens there.
    drop(job_tx);
    drop(ctrl_tx);
    let grace = Duration::from_millis(grace_ms);
    if !writer_joined {
        match tokio::time::timeout(grace, &mut writer).await {
            Ok(Ok(())) => {}
            // A panic here is not a clean drain: results the coordinator was
            // waiting for never went out. Surfaced so the process exits 70.
            Ok(Err(e)) => writer_failure = Some(join_error_message(e)),
            Err(_) => {
                tracing::warn!(
                    "outbound writer did not finish within the shutdown grace window; aborting it"
                );
                // Aborting drops the task, and with it the result receiver.
                // That is what unparks a sampler worker blocked in
                // `blocking_send`, which is what makes the join below bounded
                // for anything short of a backend stuck inside `sample`.
                writer.abort();
                let _ = (&mut writer).await;
            }
        }
    }
    // Wait out the grace window without blocking the runtime, so a worker that
    // finishes normally costs nothing, then join it. The join is not optional:
    // `sample_stream` runs on a borrow of the caller's `Sampler`, and this
    // function is reachable from C through `quip_solver_run`, where the caller
    // frees the `user_data` its sample callback closes over the moment the call
    // returns. A worker still running at that point calls into freed memory.
    //
    // This thread used to be abandoned when it overran the window, which traded
    // that use-after-free for a shorter shutdown. The trade is the wrong way
    // round: a slow exit is visible and survivable, a use-after-free is neither.
    //
    // What keeps the join bounded in practice: the outbound writer is finished
    // or aborted by now, so its `res_rx` is dropped and no `blocking_send` can
    // park — the case the old comment was written for. What remains unbounded
    // is a backend wedged inside `sample`, and the escalation below is the only
    // lever this layer has over it. If that backend ignores the token too, the
    // process parks here with the error below in the log, waiting for SIGKILL.
    let sampler_deadline = tokio::time::Instant::now() + grace;
    while !sampler_thread.is_finished() && tokio::time::Instant::now() < sampler_deadline {
        tokio::time::sleep(SAMPLER_JOIN_POLL).await;
    }
    if !sampler_thread.is_finished() {
        tracing::error!(
            "sampler thread did not finish within the shutdown grace window; \
             cancelling every generation and waiting for it to return"
        );
        // Cooperative stop: a backend that owns its sweep loop polls the token
        // at its checkpoints, so raising the watermark past every generation
        // abandons the work in flight.
        cancel.cancel_through(u64::MAX);
    }
    if let Err(panic) = sampler_thread.join() {
        let payload = panic_payload_message(&*panic);
        tracing::error!(panic = %payload, "sampler thread panicked");
        return Err(SessionExit::new(
            ExitCode::InternalFatal,
            format!("sampler thread panicked: {payload}"),
        )
        .into());
    }

    drop(tx);
    let drain = async {
        while inbound.message().await?.is_some() {}
        Ok::<(), tonic::Status>(())
    };
    // Best-effort: the session is over either way, and a peer that errors or
    // stalls while we drain changes nothing. Logged rather than dropped so a
    // coordinator that consistently fails here is visible at all.
    match tokio::time::timeout(grace, drain).await {
        Ok(Ok(())) => {}
        Ok(Err(status)) => tracing::debug!(%status, "final inbound drain ended with an error"),
        Err(_) => tracing::debug!("final inbound drain did not finish within the grace window"),
    }

    // Ordered by severity: the outbound half failing means results never
    // reached the coordinator, a protocol failure means the session was never
    // valid, and a device fault means this process cannot serve more work.
    if let Some(detail) = writer_failure {
        tracing::error!(detail, "outbound writer did not finish cleanly");
        return Err(SessionExit::new(ExitCode::InternalFatal, detail).into());
    }
    if let Some(e) = handshake_error {
        return Err(e.into());
    }
    if let Some(e) = protocol_failure {
        tracing::error!(detail = %e, "session ended without a clean shutdown");
        return Err(e.into());
    }
    if device_faulted.load(Ordering::Relaxed) {
        return Err(SessionExit::new(
            ExitCode::InternalFatal,
            "device fault: the backend reported an unrecoverable state",
        )
        .into());
    }
    Ok(())
}

/// Describe a writer-task `JoinError` for the log and the session error.
fn join_error_message(e: tokio::task::JoinError) -> String {
    if e.is_panic() {
        format!(
            "outbound writer panicked: {}",
            panic_payload_message(&*e.into_panic())
        )
    } else {
        format!("outbound writer did not run to completion: {e}")
    }
}

/// Format a `JoinHandle` panic payload for logging / error messages.
fn panic_payload_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

fn map_err_to_exit(err: Box<dyn std::error::Error>, backend: &str) -> ExitCode {
    // Prefer the canonical `SessionError -> ExitCode` mapping (quip-protocol)
    // over a hand-rolled match, so real miners exit the same code as the mock
    // reference (e.g. BadWelcome -> ConfigInvalid/64, not InternalFatal/70).
    let err = match err.downcast::<SessionError>() {
        Ok(se) => return ExitCode::from(*se),
        Err(err) => err,
    };
    // Ends this session layer chose itself, carrying their own exit code.
    let err = match err.downcast::<SessionExit>() {
        Ok(exit) => return exit.code,
        Err(err) => err,
    };
    // Type-erased fallback: the error crossed a boundary that lost the
    // concrete `SessionError` (e.g. tonic::Status::into()). Recover only the
    // two documented exit codes we can identify from the message.
    let msg = err.to_string();
    if msg.contains("QUIP_SESSION_TOKEN") || msg.contains("session token") {
        return ExitCode::TokenRejected;
    }
    if msg.contains("unexpected protocol version") {
        return ExitCode::ConfigInvalid;
    }
    tracing::error!("quip-miner-{backend} fatal: {err}");
    ExitCode::InternalFatal
}

/// Miner entry point returning the numeric exit code from SPEC section 2.
///
/// `open` opens the device and builds the [`Sampler`]; it runs for `--check`
/// (result discarded), `--solve`, and session mode. `--capabilities` never
/// calls it.
///
/// Prefer [`run`] from a Rust `main`. This variant exists because
/// `std::process::ExitCode` cannot be read back into a number, which a foreign
/// function interface has to do to return the code to its own caller.
pub fn run_code<S: Sampler>(
    id: BackendIdentity,
    common: &CommonArgs,
    open: impl FnOnce() -> Result<S, OpenError>,
) -> ExitCode {
    // Install the subscriber before anything else can log. `--capabilities`
    // writes JSON to stdout and must stay parseable, but the subscriber writes
    // to stderr, so installing first is safe for it too.
    if let Err(e) = crate::logging::init(&common.log_level) {
        #[expect(
            clippy::print_stderr,
            reason = "the subscriber failed to install, so tracing would discard this"
        )]
        {
            eprintln!("quip-miner-{}: {e}", id.backend);
        }
        return ExitCode::ConfigInvalid;
    }

    if common.capabilities {
        return print_capabilities::<S>(&id);
    }
    if common.solve {
        let sampler = match open() {
            Ok(s) => s,
            Err(OpenError(e)) => {
                tracing::error!("[quip-solver-{}] cannot open device: {e}", id.backend);
                return ExitCode::EnvIncompatible;
            }
        };
        let mut input = Vec::new();
        if let Err(e) = std::io::Read::read_to_end(&mut std::io::stdin(), &mut input) {
            tracing::error!("[quip-solver-{}] cannot read stdin: {e}", id.backend);
            return ExitCode::ConfigInvalid;
        }
        if serde_json::from_slice::<crate::driver::ProblemJson>(&input).is_err() {
            tracing::error!(
                "[quip-solver-{}] malformed problem JSON on stdin",
                id.backend
            );
            return ExitCode::ConfigInvalid;
        }
        return match crate::driver::solve(&sampler, &input) {
            Ok(mut bytes) => {
                // `println!` supplied this before the broken-pipe fix. A CLI
                // that emits a JSON document should end it with a newline.
                bytes.push(b'\n');
                write_and_map(&mut std::io::stdout(), &bytes)
            }
            // The stdin pre-parse above already rejects malformed JSON with
            // ConfigInvalid, so Malformed is unreachable here in practice;
            // this still maps it correctly for a direct caller of `solve`.
            Err(crate::driver::SolveError::Malformed(detail)) => {
                tracing::error!(
                    "[quip-solver-{}] malformed problem JSON on stdin: {detail}",
                    id.backend
                );
                ExitCode::ConfigInvalid
            }
            Err(e @ crate::driver::SolveError::Sample(_)) => {
                tracing::error!("[quip-solver-{}] solve failed: {e}", id.backend);
                ExitCode::InternalFatal
            }
        };
    }
    if common.check {
        return match open() {
            Ok(_) => ExitCode::Clean,
            Err(e) => {
                tracing::error!("{} check failed: {}", id.backend, e.0);
                ExitCode::EnvIncompatible
            }
        };
    }

    let Some(uri) = common.quip_coordinator.clone() else {
        tracing::error!("error: --quip-coordinator required for session mode");
        return ExitCode::ConfigInvalid;
    };
    let miner_id = common
        .miner_id
        .clone()
        .unwrap_or_else(|| format!("{}-0", id.backend));

    let sampler = match open() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to open {} device: {}", id.backend, e.0);
            return ExitCode::EnvIncompatible;
        }
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("failed to start tokio runtime: {e}");
            return ExitCode::InternalFatal;
        }
    };

    match rt.block_on(run_session(
        &uri,
        &miner_id,
        &id,
        Arc::new(sampler),
        common.sweeps_per_beta,
    )) {
        Ok(()) => ExitCode::Clean,
        Err(e) => map_err_to_exit(e, id.backend),
    }
}

/// Miner entry point. Dispatches `--capabilities`/`--solve`/`--check`/session mode.
///
/// Thin wrapper over [`run_code`] for use as a Rust `main` return value.
pub fn run<S: Sampler>(
    id: BackendIdentity,
    common: &CommonArgs,
    open: impl FnOnce() -> Result<S, OpenError>,
) -> StdExitCode {
    StdExitCode::from(run_code(id, common, open) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One outbound writer wired to fresh channels, for the writer tests.
    struct WriterHarness {
        res_tx: mpsc::Sender<StreamResult>,
        ctrl_tx: mpsc::Sender<MinerMsg>,
        out_rx: mpsc::Receiver<MinerMsg>,
        pending: PendingParams,
        cancel: CancelToken,
        writer: tokio::task::JoinHandle<()>,
    }

    fn spawn_writer(depth: usize) -> WriterHarness {
        let (tx, out_rx) = mpsc::channel::<MinerMsg>(depth);
        let (res_tx, res_rx) = mpsc::channel::<StreamResult>(depth);
        let (ctrl_tx, ctrl_rx) = mpsc::channel::<MinerMsg>(depth);
        let pending: PendingParams = Arc::new(StdMutex::new(HashMap::new()));
        let cancel = CancelToken::default();
        let writer = tokio::spawn(outbound_writer(
            tx,
            res_rx,
            ctrl_rx,
            WriterContext {
                pending: Arc::clone(&pending),
                jobs_done: Arc::new(AtomicU64::new(0)),
                device_faulted: Arc::new(AtomicBool::new(false)),
                cancel: cancel.clone(),
                backend: "test",
            },
        ));
        WriterHarness {
            res_tx,
            ctrl_tx,
            out_rx,
            pending,
            cancel,
            writer,
        }
    }

    fn completed_result(job_id: u8) -> StreamResult {
        StreamResult {
            job_id: vec![job_id],
            outcome: StreamOutcome::Completed(Ok(vec![crate::SamplerResult {
                spins: vec![1i8, -1],
                energy_milli: -1000,
            }])),
            device_access_time_us: 0,
        }
    }

    fn pending_job(watermark: Option<u64>) -> PendingJob {
        PendingJob {
            num_reads: 1,
            num_sweeps: 1,
            started: std::time::Instant::now(),
            max_energy_milli: None,
            min_solutions: 0,
            watermark,
        }
    }

    /// The writer waits, rather than spins, once the read loop is gone while the
    /// sampler still holds `res_tx`.
    ///
    /// That window is every shutdown: `run_session` drops `ctrl_tx` first and
    /// the sampler keeps `res_tx` until its last in-flight job finishes. A
    /// closed `ctrl_rx` completes `recv()` with `None` immediately and forever,
    /// so a `select!` that leaves the branch enabled burns a core for the whole
    /// drain.
    ///
    /// The paused clock is what makes that observable rather than a matter of
    /// opinion: tokio advances a paused clock only while every task is idle, so
    /// a spinning writer stops the sleep below from ever returning and this test
    /// hangs instead of passing.
    #[tokio::test(start_paused = true)]
    async fn writer_parks_when_the_read_loop_closes_before_the_sampler() {
        let WriterHarness {
            res_tx,
            ctrl_tx,
            mut out_rx,
            writer,
            ..
        } = spawn_writer(16);

        // Read loop exits, as it does on Shutdown; the sampler is still working.
        drop(ctrl_tx);
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert!(
            !writer.is_finished(),
            "writer must keep draining results until the sampler releases res_tx"
        );

        // And it is still serving the result side while the control side is shut.
        res_tx
            .send(StreamResult {
                job_id: vec![1, 2, 3],
                outcome: StreamOutcome::Cancelled,
                device_access_time_us: 0,
            })
            .await
            .unwrap();
        let msg = out_rx.recv().await.expect("credit refund reached the wire");
        assert!(
            matches!(msg.msg, Some(miner_msg::Msg::JobRequest(_))),
            "a Cancelled result refunds its credit and sends nothing else"
        );

        // Sampler finishes: both inputs are done, so the writer returns.
        drop(res_tx);
        tokio::time::timeout(Duration::from_secs(5), writer)
            .await
            .expect("writer returned once both inputs closed")
            .expect("writer did not panic");
    }

    fn test_identity() -> BackendIdentity {
        BackendIdentity {
            backend: "mock",
            algorithm: "sa",
            max_nodes: 100,
            max_edges: 200,
            features: &["streaming"],
            adapt: crate::adapt::AdaptBounds {
                min_sweeps: 1,
                max_sweeps: 1,
                min_reads: 1,
                max_reads: 1,
                reads_solution_min_factor: 1,
                reads_solution_max_factor: 1,
                reads_solution_floor_factor: 0,
            },
        }
    }

    #[test]
    fn capabilities_constructor_matches_the_printer() {
        let id = test_identity();
        let c = capabilities(&id, 4);
        let v: serde_json::Value =
            serde_json::from_str(&capabilities_json_from(&c)).expect("printer emits JSON");
        assert_eq!(v.get("backend"), Some(&serde_json::json!(c.backend)));
        assert_eq!(v.get("algorithm"), Some(&serde_json::json!(c.algorithm)));
        assert_eq!(v.get("maxNodes"), Some(&serde_json::json!(c.max_nodes)));
        assert_eq!(v.get("maxEdges"), Some(&serde_json::json!(c.max_edges)));
        assert_eq!(
            v.get("protocolVersion"),
            Some(&serde_json::json!(c.protocol_version))
        );
        assert_eq!(
            v.get("streamWidth"),
            Some(&serde_json::json!(c.stream_width))
        );
        assert_eq!(v.get("features"), Some(&serde_json::json!(["streaming"])));
        assert_eq!(
            v.get("supportedKinds"),
            Some(&serde_json::json!(["ISING_SAMPLE"]))
        );
        assert_eq!(c.supported_kinds, vec![JobKind::IsingSample as i32]);
        assert_eq!(c.features, vec!["streaming"]);
        assert_eq!(c.native_topology_hash, None);
    }

    #[test]
    fn capabilities_json_escapes_quotes_in_features() {
        let id = BackendIdentity {
            backend: "mock",
            algorithm: "sa",
            max_nodes: 1,
            max_edges: 1,
            features: &[r#"has"quote"#],
            adapt: crate::adapt::AdaptBounds {
                min_sweeps: 1,
                max_sweeps: 1,
                min_reads: 1,
                max_reads: 1,
                reads_solution_min_factor: 1,
                reads_solution_max_factor: 1,
                reads_solution_floor_factor: 0,
            },
        };
        let json = capabilities_json_from(&capabilities(&id, 1));
        let v: serde_json::Value =
            serde_json::from_str(&json).expect("quoted feature must produce valid JSON");
        assert_eq!(v.get("features"), Some(&serde_json::json!(["has\"quote"])));
    }

    #[test]
    fn native_topology_hash_serializes_as_base64_string() {
        // `--capabilities` has no path that sets this field. Build the JSON
        // view from a Capabilities that carries a hash.
        let mut c = capabilities(&test_identity(), 1);
        c.native_topology_hash = Some(vec![1, 2, 3]);
        let json = capabilities_json_from(&c);
        let v: serde_json::Value = serde_json::from_str(&json).expect("printer emits JSON");
        let hash = v.get("nativeTopologyHash").expect("field present");
        assert!(
            hash.is_string(),
            "protobuf JSON maps bytes to a base64 string"
        );
        assert!(!hash.is_array(), "must not emit a JSON number array");
        assert_eq!(hash, &serde_json::json!("AQID"));
    }

    struct FailWrite(std::io::ErrorKind);

    impl Write for FailWrite {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(self.0, "test write failure"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A continuously non-empty result queue must not starve control replies.
    ///
    /// `biased` puts results first, which is right until the sampler is fast
    /// enough that the result branch is always ready. A coordinator that sends
    /// `Cancel` and waits for the `Status` ack, or `Ping` and waits for the
    /// reply, then waits behind every result the miner has yet to produce. With
    /// the queue pre-filled, the unbounded version answers only after all 64.
    #[tokio::test]
    async fn control_replies_are_not_starved_by_a_busy_result_queue() {
        let harness = spawn_writer(128);
        let WriterHarness {
            res_tx,
            ctrl_tx,
            mut out_rx,
            writer,
            ..
        } = harness;

        // Fill the result queue, then queue one control reply behind it.
        for i in 0..64u8 {
            res_tx.send(completed_result(i)).await.expect("send result");
        }
        ctrl_tx
            .send(status_msg("miner", 0, 0.0, 0))
            .await
            .expect("send status");

        // Position of the Status among the outbound messages. Each Cancelled
        // result emits one JobRequest, so the index counts results served
        // before the control queue was.
        let mut index = 0_u32;
        loop {
            let msg = out_rx.recv().await.expect("writer keeps sending");
            if matches!(msg.msg, Some(miner_msg::Msg::Status(_))) {
                break;
            }
            index = index.saturating_add(1);
        }
        // An absolute bound, deliberately not written in terms of
        // RESULT_BATCH: expressing it relative to the constant would let a
        // raised constant make this assertion vacuous, which is exactly the
        // regression it exists to catch.
        assert!(
            index <= 16,
            "a control reply waited behind {index} results; RESULT_BATCH is {RESULT_BATCH}"
        );

        drop(res_tx);
        drop(ctrl_tx);
        let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
    }

    /// SPEC section 5, defence in depth: a `Result` whose generation was
    /// cancelled must not reach the wire even when the backend produced one.
    /// A backend that only checks the watermark at dequeue does exactly that.
    #[tokio::test]
    async fn the_writer_drops_a_result_for_a_cancelled_generation() {
        let WriterHarness {
            res_tx,
            ctrl_tx,
            mut out_rx,
            pending,
            cancel,
            writer,
        } = spawn_writer(16);

        {
            let mut p = pending.lock().expect("fresh mutex");
            let _ = p.insert(vec![7], pending_job(Some(4)));
        }
        cancel.cancel_through(4);
        res_tx.send(completed_result(7)).await.expect("send result");

        let first = out_rx.recv().await.expect("writer replies");
        assert!(
            matches!(first.msg, Some(miner_msg::Msg::JobRequest(jr)) if jr.credits == 1),
            "a cancelled job refunds exactly one credit and sends no Result"
        );

        // Nothing else follows: no Result, no Reject.
        drop(res_tx);
        drop(ctrl_tx);
        assert!(
            out_rx.recv().await.is_none(),
            "a cancelled generation must produce no further outbound message"
        );
        let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
    }

    /// The same job, with nothing cancelled, still reports its `Result`. Without
    /// this the test above passes on a writer that drops every result.
    #[tokio::test]
    async fn the_writer_keeps_a_result_for_a_live_generation() {
        let WriterHarness {
            res_tx,
            ctrl_tx,
            mut out_rx,
            pending,
            writer,
            ..
        } = spawn_writer(16);

        {
            let mut p = pending.lock().expect("fresh mutex");
            let _ = p.insert(vec![7], pending_job(Some(4)));
        }
        res_tx.send(completed_result(7)).await.expect("send result");

        let first = out_rx.recv().await.expect("writer replies");
        assert!(
            matches!(first.msg, Some(miner_msg::Msg::Result(_))),
            "a live generation must report its Result"
        );
        drop(res_tx);
        drop(ctrl_tx);
        let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
    }

    /// A panicking writer must not read as a clean session.
    ///
    /// The `JoinHandle` result used to be discarded on both paths, so a miner
    /// whose outbound half died mid-session still exited 0 and its supervisor
    /// saw a normal stop.
    #[tokio::test]
    async fn a_writer_panic_maps_to_internal_fatal() {
        let handle = tokio::spawn(async { panic!("writer exploded") });
        let err = handle.await.expect_err("the task panicked");
        let detail = join_error_message(err);
        assert!(
            detail.contains("panicked") && detail.contains("writer exploded"),
            "the panic payload must survive into the message: {detail}"
        );
        assert_eq!(
            map_err_to_exit(
                SessionExit::new(ExitCode::InternalFatal, detail).into(),
                "test"
            ),
            ExitCode::InternalFatal
        );
    }

    /// Every `SessionError` must reach its documented exit code after being
    /// boxed as `dyn Error`, which is the only shape `run_code` ever sees.
    #[test]
    fn session_errors_round_trip_through_a_boxed_error() {
        for (err, want) in [
            (SessionError::MissingToken, ExitCode::TokenRejected),
            (SessionError::BadWelcome(0), ExitCode::ConfigInvalid),
            (SessionError::BadWelcome(2), ExitCode::ConfigInvalid),
        ] {
            let boxed: Box<dyn std::error::Error> = Box::new(err);
            assert_eq!(map_err_to_exit(boxed, "test"), want, "for {err:?}");
        }
    }

    /// A `SessionExit` carries its own code, so these exits do not depend on
    /// the wording of anyone's `Display`.
    #[test]
    fn session_exits_carry_their_own_code() {
        for code in [
            ExitCode::TokenRejected,
            ExitCode::InternalFatal,
            ExitCode::ConfigInvalid,
        ] {
            let boxed: Box<dyn std::error::Error> =
                SessionExit::new(code, "coordinator went away").into();
            assert_eq!(map_err_to_exit(boxed, "test"), code);
        }
    }

    /// The text-matching fallback still has to work: errors from crates this
    /// one does not own arrive with the concrete type erased. These strings are
    /// the two `SessionError` `Display` forms, so a reword there fails here
    /// rather than silently turning a token failure into exit 70.
    #[test]
    fn the_type_erased_fallback_recovers_the_documented_codes() {
        let missing: Box<dyn std::error::Error> = SessionError::MissingToken.to_string().into();
        assert_eq!(map_err_to_exit(missing, "test"), ExitCode::TokenRejected);

        let bad: Box<dyn std::error::Error> = SessionError::BadWelcome(9).to_string().into();
        assert_eq!(map_err_to_exit(bad, "test"), ExitCode::ConfigInvalid);

        // Anything else is an internal fault, not a guess.
        let other: Box<dyn std::error::Error> = "transport closed".to_owned().into();
        assert_eq!(map_err_to_exit(other, "test"), ExitCode::InternalFatal);
    }

    /// `--capabilities` and the in-session reply are one message, so they carry
    /// one `stream_width`. The flag used to hardcode `1` while the session
    /// reported the live width.
    #[test]
    fn both_capabilities_answers_carry_the_declared_stream_width() {
        struct WideSampler;
        impl Sampler for WideSampler {
            fn sample(
                &self,
                _graph: &crate::IsingGraph,
                _params: &crate::ising::SampleParams,
            ) -> Result<Vec<crate::SamplerResult>, crate::SampleError> {
                Ok(vec![])
            }
            fn stream_width(&self) -> usize {
                8
            }
            fn declared_stream_width() -> u32 {
                8
            }
        }

        let id = test_identity();
        // What GetCapabilities replies with.
        let reply = advertised_capabilities::<WideSampler>(&id);
        assert_eq!(reply.stream_width, 8);
        // What `--capabilities` prints, from the same function.
        let v: serde_json::Value =
            serde_json::from_str(&capabilities_json_from(&reply)).expect("printer emits JSON");
        assert_eq!(v.get("streamWidth"), Some(&serde_json::json!(8)));
        // And the features the identity declares travel with it.
        assert_eq!(reply.features, vec!["streaming"]);
    }

    /// A backend whose width depends on the opened device declares 0. The
    /// static answer keeps the 0; the in-session reply reports the resolved
    /// width.
    #[test]
    fn a_device_dependent_width_declaration_resolves_in_session() {
        struct DeviceWidthSampler;
        impl Sampler for DeviceWidthSampler {
            fn sample(
                &self,
                _graph: &crate::IsingGraph,
                _params: &crate::ising::SampleParams,
            ) -> Result<Vec<crate::SamplerResult>, crate::SampleError> {
                Ok(vec![])
            }
            fn stream_width(&self) -> usize {
                6
            }
            fn declared_stream_width() -> u32 {
                0
            }
        }

        let id = test_identity();
        let flag = advertised_capabilities::<DeviceWidthSampler>(&id);
        assert_eq!(flag.stream_width, 0);
        let reply = session_capabilities::<DeviceWidthSampler>(&id, 6);
        assert_eq!(reply.stream_width, 6);
    }

    #[test]
    fn write_and_map_treats_broken_pipe_as_clean_and_other_errors_as_fatal() {
        // Bytes must be non-empty: write_all returns Ok on an empty slice
        // without calling write.
        assert_eq!(
            write_and_map(&mut FailWrite(std::io::ErrorKind::BrokenPipe), b"x"),
            ExitCode::Clean
        );
        assert_eq!(
            write_and_map(&mut FailWrite(std::io::ErrorKind::Other), b"x"),
            ExitCode::InternalFatal
        );
    }
}
