//! The `linear-schedule` case grades the ladder a solver reports.

use quip_solver_conformance::solve::{drive_solve, CaseOutcome};
use std::path::PathBuf;
use std::process::Command;

/// Build and locate the example named `name`.
#[expect(
    clippy::expect_used,
    reason = "test helper: current_exe and the example build are setup steps"
)]
fn example(name: &str) -> PathBuf {
    let mut profile = std::env::current_exe().expect("test exe path");
    let _ = profile.pop(); // deps/
    let _ = profile.pop(); // <profile>/
    let path = profile.join("examples").join(name);
    if !path.is_file() {
        let profile_name = match profile.file_name().and_then(|n| n.to_str()) {
            Some("debug") | None => "dev",
            Some(name) => name,
        };
        let status = Command::new(env!("CARGO"))
            .args(["build", "--example", name, "-p", "quip-solver-conformance"])
            .args(["--profile", profile_name])
            .status()
            .expect("cargo build --example");
        assert!(status.success(), "failed to build {name}");
    }
    path
}

#[tokio::test]
async fn a_solver_that_reports_the_linear_ladder_passes() {
    let bin = example("mock_ladder_linear");
    let report = drive_solve(&bin.to_string_lossy()).await;
    assert_eq!(
        report.outcome("linear-schedule"),
        Some(&CaseOutcome::Passed),
        "{}",
        report.summary()
    );
}

#[tokio::test]
async fn a_solver_that_lists_linear_but_reports_a_geometric_ladder_fails() {
    let bin = example("mock_ladder_geometric");
    let report = drive_solve(&bin.to_string_lossy()).await;
    match report.outcome("linear-schedule") {
        Some(CaseOutcome::Failed(why)) => {
            assert!(
                why.contains("kind geometric") || why.contains("\"geometric\""),
                "{why}"
            );
        }
        other => panic!("expected a failure, got {other:?}\n{}", report.summary()),
    }
}
