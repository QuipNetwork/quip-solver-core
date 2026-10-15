//! The `--solve` gate's QPU cases, run against the crate's stand-in annealer.

use quip_solver_conformance::solve::{drive_solve, CaseOutcome};
use std::process::Command;

fn mock_qpu_solver() -> String {
    #[expect(
        clippy::expect_used,
        reason = "test helper: cargo build failure is a setup error"
    )]
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "--example",
            "mock_qpu_solver",
            "-p",
            "quip-solver-conformance",
        ])
        .status()
        .expect("cargo build --example");
    assert!(status.success(), "failed to build mock_qpu_solver");
    #[expect(
        clippy::expect_used,
        reason = "test helper: current_exe is always available under cargo test"
    )]
    let mut p = std::env::current_exe().expect("test exe path");
    let _ = p.pop();
    let _ = p.pop();
    p.push("examples");
    p.push("mock_qpu_solver");
    p.to_string_lossy().into_owned()
}

/// An annealer that lists `anneal-time` and `embed` is conformant, and both
/// cases run and pass rather than skip.
#[tokio::test]
async fn an_annealer_passes_the_anneal_time_and_embed_cases() {
    let report = drive_solve(&mock_qpu_solver()).await;
    assert!(report.is_conformant(), "{}", report.summary());
    for name in ["solve", "same-seed", "anneal-time", "embed"] {
        assert_eq!(
            report.outcome(name),
            Some(&CaseOutcome::Passed),
            "{name}\n{}",
            report.summary()
        );
    }
}
