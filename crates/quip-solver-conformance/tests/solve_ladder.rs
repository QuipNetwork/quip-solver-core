//! The `linear-schedule` case grades the ladder a solver reports.

use quip_solver_conformance::solve::{drive_solve, CaseOutcome};

mod common;
use common::example;

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

#[tokio::test]
async fn a_solver_that_lists_linear_but_leaves_the_ladder_to_the_sdk_fails() {
    let bin = example("mock_ladder_sdk");
    let report = drive_solve(&bin.to_string_lossy()).await;
    match report.outcome("linear-schedule") {
        Some(CaseOutcome::Failed(why)) => assert!(why.contains("reported_by"), "{why}"),
        other => panic!("expected a failure, got {other:?}\n{}", report.summary()),
    }
}
