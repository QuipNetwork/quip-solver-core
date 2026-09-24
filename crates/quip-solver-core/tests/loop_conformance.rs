//! Protocol conformance for the generic session loop, driven by a mock sampler.
//!
//! Exercises the loop without any real backend: handshake, Ready, credits, job
//! results, each Reject reason, and clean exit.

use quip_proto::v1::RejectReason;
use quip_solver_conformance::driver::{
    drive_miner, drive_miner_bad_welcome, drive_miner_close_after_welcome,
    drive_miner_close_before_welcome, drive_miner_one_job,
};
use std::process::Command;

/// Build the named example and return its binary path.
fn example_bin(name: &str) -> String {
    #[expect(
        clippy::expect_used,
        reason = "test helper: cargo build failure is a setup error"
    )]
    let status = Command::new(env!("CARGO"))
        .args(["build", "--example", name, "-p", "quip-solver-core"])
        .status()
        .expect("cargo build --example");
    assert!(status.success(), "failed to build example {name}");
    #[expect(
        clippy::expect_used,
        reason = "test helper: current_exe is always available under cargo test"
    )]
    let mut p = std::env::current_exe().expect("test exe path");
    let _ = p.pop(); // deps/
    let _ = p.pop(); // <profile>/
    p.push("examples");
    p.push(name);
    p.to_string_lossy().into_owned()
}

fn unique_socket(tag: &str) -> String {
    let nanos = {
        #[expect(
            clippy::unwrap_used,
            reason = "test helper: system clock is after UNIX_EPOCH"
        )]
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    };
    format!("/tmp/quip-core-{tag}-{}-{nanos}.sock", std::process::id())
}

/// A solver that uses warm starts is graded on them and passes.
#[tokio::test]
async fn warm_sampler_passes_the_initial_spins_grade() {
    let bin = example_bin("mock_sampler_warm");
    let socket = unique_socket("warm");
    let report = drive_miner(&bin, &format!("unix://{socket}")).await;
    assert!(report.advertises_initial_spins(), "{report:?}");
    assert!(report.warm_start_conformant(), "{}", report.summary());
    assert!(report.is_conformant(), "{}", report.summary());
}

/// A solver that advertises `initial-spins` but samples cold fails the grade:
/// it never reaches the proof ring's planted ground state.
#[tokio::test]
async fn a_solver_that_ignores_its_seeds_fails_the_initial_spins_grade() {
    let bin = example_bin("mock_sampler_warm_ignored");
    let socket = unique_socket("warm-ignored");
    let report = drive_miner(&bin, &format!("unix://{socket}")).await;
    assert!(report.advertises_initial_spins(), "{report:?}");
    assert!(
        report.has_reject(b"job-bad-seed", RejectReason::Malformed),
        "core rejects the malformed seed whatever the sampler does"
    );
    assert!(!report.warm_start_conformant(), "{}", report.summary());
    assert!(!report.is_conformant());
}

#[tokio::test]
async fn mock_sampler_passes_loop_conformance() {
    let bin = example_bin("mock_sampler_miner");
    let socket = unique_socket("loop");
    let report = drive_miner(&bin, &format!("unix://{socket}")).await;

    assert!(report.handshake_ok, "handshake failed");
    assert!(report.ready_received, "Ready not received after Configure");
    assert!(
        !report.job_request_credits.is_empty(),
        "miner never requested credits"
    );
    assert!(
        report.result_job_ids().iter().any(|id| id == b"job-1"),
        "missing result for job-1: {:?}",
        report.result_job_ids()
    );
    assert!(
        report.result_job_ids().iter().any(|id| id == b"job-2"),
        "missing result for job-2: {:?}",
        report.result_job_ids()
    );
    assert!(
        report.result_job_ids().iter().any(|id| id == b"job-hash"),
        "missing result for topology-hash job-hash (cache/resolve regression): {:?}",
        report.result_job_ids()
    );
    assert!(
        report.has_reject(b"job-bad-h", RejectReason::Malformed),
        "missing MALFORMED reject for job-bad-h: {:?}",
        report.rejects
    );
    assert!(
        report.has_reject(b"job-gate", RejectReason::UnsupportedKind),
        "missing UNSUPPORTED_KIND reject for job-gate: {:?}",
        report.rejects
    );
    assert!(
        report.has_reject(b"job-old", RejectReason::Expired),
        "missing EXPIRED reject for job-old: {:?}",
        report.rejects
    );
    assert_eq!(report.exit_code, 0, "clean shutdown expected");

    // The granular assertions above are a diagnostic aid, not the gate: each
    // one names the axis that broke. `is_conformant()` is the gate a
    // third-party solver is actually held to, and asserting only a subset of
    // it let requirements (the Cancel ack, the job-bad-j reject) drift
    // unchecked. Assert the whole verdict too, so the two can never diverge.
    assert!(report.is_conformant(), "{report:?}");
}

#[tokio::test]
async fn capped_sampler_rejects_too_large() {
    let bin = example_bin("mock_sampler_capped");
    let socket = unique_socket("capped");
    let report = drive_miner(&bin, &format!("unix://{socket}")).await;

    assert!(report.handshake_ok, "handshake failed");
    // With max_reads()==0, every job (num_reads ≥ 1) is rejected TooLarge.
    assert!(
        report.has_reject(b"job-1", RejectReason::TooLarge),
        "expected TooLarge for job-1: {:?}",
        report.rejects
    );
    assert!(
        report.result_job_ids().is_empty(),
        "capped sampler must not produce results: {:?}",
        report.result_job_ids()
    );
    assert_eq!(report.exit_code, 0, "clean shutdown expected");
}

#[tokio::test]
async fn a_bad_welcome_ends_the_session_with_config_invalid() {
    let bin = example_bin("mock_sampler_miner");
    let socket = unique_socket("bad-welcome");
    let report = drive_miner_bad_welcome(&bin, &format!("unix://{socket}")).await;

    assert!(report.handshake_ok, "handshake failed: {report:?}");
    assert_eq!(
        report.exit_code, 64,
        "a Welcome with the wrong protocol version must exit ConfigInvalid: {report:?}"
    );
    assert!(
        report.result_job_ids().is_empty(),
        "a miner that rejected the Welcome must not have run jobs: {:?}",
        report.result_job_ids()
    );
}

/// The other half of the bad-Welcome contract: the miner must say *why* it is
/// leaving before it leaves.
///
/// From the coordinator's side, a bare exit 64 is indistinguishable from a
/// crash during startup — the operator sees a disconnect and no reason.
/// `drive_miner_bad_welcome`'s documentation has always claimed a conformant
/// miner sends `Fatal` first, the driver grades it
/// (`DriverReport::bad_welcome_conformant`), and the session loop now sends
/// `Fatal { exit_code: 64 }` before returning `SessionError::BadWelcome`.
#[tokio::test]
async fn a_bad_welcome_is_explained_before_the_exit() {
    let bin = example_bin("mock_sampler_miner");
    let socket = unique_socket("bad-welcome-fatal");
    let report = drive_miner_bad_welcome(&bin, &format!("unix://{socket}")).await;

    assert!(
        report.fatal.is_some(),
        "a conformant miner sends Fatal before exiting ConfigInvalid: {report:?}"
    );
    assert!(report.bad_welcome_conformant(), "{report:?}");
}

/// A coordinator that vanishes mid-session is not a clean end.
///
/// `Shutdown` is the only clean end to a session. A stream that simply stops
/// means the coordinator was lost with work still in flight, and a miner that
/// reports exit 0 for it tells its supervisor nothing went wrong.
#[tokio::test]
async fn a_coordinator_lost_after_welcome_exits_internal_fatal() {
    let bin = example_bin("mock_sampler_miner");
    let socket = unique_socket("close-after-welcome");
    let report = drive_miner_close_after_welcome(&bin, &format!("unix://{socket}")).await;

    assert!(report.handshake_ok, "handshake failed: {report:?}");
    assert_eq!(
        report.exit_code, 70,
        "a coordinator lost after Welcome must exit InternalFatal, not report success: {report:?}"
    );
    assert!(
        report
            .stderr
            .contains("coordinator closed the session stream without Shutdown"),
        "{}",
        report.stderr
    );
    assert!(report.close_after_welcome_conformant(), "{report:?}");
}

/// The other close point, which must not collapse into the first.
///
/// A coordinator that drops the connection before it ever answers `Hello` has
/// refused the session; from the miner's side that is indistinguishable from a
/// rejected token. Reporting it as an internal fault sends the operator to the
/// wrong place.
#[tokio::test]
async fn a_coordinator_lost_before_welcome_exits_token_rejected() {
    let bin = example_bin("mock_sampler_miner");
    let socket = unique_socket("close-before-welcome");
    let report = drive_miner_close_before_welcome(&bin, &format!("unix://{socket}")).await;

    assert!(report.handshake_ok, "handshake failed: {report:?}");
    assert_eq!(
        report.exit_code, 77,
        "a coordinator lost before Welcome must exit TokenRejected: {report:?}"
    );
    assert!(
        report.stderr.contains("the session token was rejected"),
        "{}",
        report.stderr
    );
    assert!(report.close_before_welcome_conformant(), "{report:?}");
}

#[tokio::test]
async fn a_device_fault_ends_the_session_instead_of_requesting_more_work() {
    let bin = example_bin("mock_sampler_faulty");
    let socket = unique_socket("faulty");
    let report = drive_miner_one_job(&bin, &format!("unix://{socket}")).await;

    assert!(report.handshake_ok, "handshake failed");
    assert!(
        report.has_reject(b"job-1", RejectReason::Overloaded),
        "a DeviceFault must still reject its own job: {:?}",
        report.rejects
    );
    assert_eq!(
        report.rejects.len(),
        1,
        "a device fault must stop the session at the first reject, got {:?}",
        report.rejects
    );
    assert_eq!(
        report.exit_code, 70,
        "a wedged device must exit InternalFatal, not keep accepting jobs"
    );
    assert!(report.stderr.contains("test: wedged"), "{}", report.stderr);
}
