//! The IQM cases grade the `--solve` IQM form against `supportedKinds`.

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

async fn outcomes(name: &str) -> (Vec<Option<CaseOutcome>>, String) {
    let report = drive_solve(&example(name).to_string_lossy()).await;
    let cases = ["iqm-solve", "iqm-bounds-mismatch", "iqm-unsupported"]
        .map(|case| report.outcome(case).cloned())
        .to_vec();
    (cases, report.summary())
}

fn is_failed(outcome: Option<&CaseOutcome>) -> bool {
    matches!(outcome, Some(CaseOutcome::Failed(_)))
}

fn is_skipped(outcome: Option<&CaseOutcome>) -> bool {
    matches!(outcome, Some(CaseOutcome::Skipped(_)))
}

#[tokio::test]
async fn a_solver_that_lists_iqm_and_scores_it_passes_the_iqm_cases() {
    let (cases, summary) = outcomes("mock_iqm").await;
    assert_eq!(
        cases,
        [Some(CaseOutcome::Passed), Some(CaseOutcome::Passed), None],
        "{summary}"
    );
}

#[tokio::test]
async fn a_solver_that_misreports_an_iqm_energy_fails_the_solve_case() {
    let (cases, summary) = outcomes("mock_iqm_wrong_energy").await;
    assert!(
        is_failed(cases.first().and_then(Option::as_ref)),
        "{summary}"
    );
    assert!(summary.contains("its values score"), "{summary}");
    assert_eq!(cases.get(1), Some(&Some(CaseOutcome::Passed)), "{summary}");
}

#[tokio::test]
async fn a_solver_that_solves_iqm_without_listing_it_fails_the_refusal_case() {
    let (cases, summary) = outcomes("mock_iqm_unlisted").await;
    assert!(
        is_skipped(cases.first().and_then(Option::as_ref)),
        "{summary}"
    );
    assert!(
        is_failed(cases.get(2).and_then(Option::as_ref)),
        "{summary}"
    );
}

#[tokio::test]
async fn a_solver_without_iqm_refuses_the_form_and_skips_the_iqm_cases() {
    let (cases, summary) = outcomes("mock_options_strict").await;
    assert!(
        is_skipped(cases.first().and_then(Option::as_ref)),
        "{summary}"
    );
    assert!(
        is_skipped(cases.get(1).and_then(Option::as_ref)),
        "{summary}"
    );
    assert_eq!(cases.get(2), Some(&Some(CaseOutcome::Passed)), "{summary}");
}
