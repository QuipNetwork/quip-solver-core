//! Protocol conformance for the generic session loop, driven by a mock sampler.
//!
//! Exercises the loop without any real backend: handshake, Ready, credits, job
//! results, each Reject reason, and clean exit.

use quip_solver_conformance::driver::{drive_miner, drive_miner_bad_welcome};
use quip_proto::v1::RejectReason;
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
