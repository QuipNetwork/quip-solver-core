//! The option cases grade `--solve` options against the advertised
//! `solveOptions`.

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
async fn a_solver_that_validates_its_options_passes_every_option_case() {
    let bin = example("mock_options_strict");
    let report = drive_solve(&bin.to_string_lossy()).await;
    for case in ["option-valid", "option-undeclared", "option-wrong-type"] {
        assert_eq!(
            report.outcome(case),
            Some(&CaseOutcome::Passed),
            "{case}\n{}",
            report.summary()
        );
    }
}

#[tokio::test]
async fn a_solver_that_accepts_any_option_fails_the_refusal_cases() {
    let bin = example("mock_options_lax");
    let report = drive_solve(&bin.to_string_lossy()).await;
    for case in ["option-undeclared", "option-wrong-type"] {
        assert!(
            matches!(report.outcome(case), Some(CaseOutcome::Failed(_))),
            "{case} must fail\n{}",
            report.summary()
        );
    }
    assert_eq!(
        report.outcome("option-valid"),
        Some(&CaseOutcome::Passed),
        "{}",
        report.summary()
    );
}

#[tokio::test]
async fn a_solver_without_options_skips_the_typed_cases_and_still_refuses_unknown_names() {
    let bin = example("mock_ladder_linear");
    let report = drive_solve(&bin.to_string_lossy()).await;
    // The ladder mock hand-rolls its parser with `deny_unknown_fields`, so it
    // refuses the undeclared name as a solver without options must.
    assert_eq!(
        report.outcome("option-undeclared"),
        Some(&CaseOutcome::Passed),
        "{}",
        report.summary()
    );
    for case in ["option-valid", "option-wrong-type"] {
        assert!(
            matches!(report.outcome(case), Some(CaseOutcome::Skipped(_))),
            "{case}\n{}",
            report.summary()
        );
    }
}
