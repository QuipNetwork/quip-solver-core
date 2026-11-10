//! The `--solve` gate from quip-solver-conformance, run against the SDK's own
//! test-support miners. A green run here means the SDK side of SPEC section 2
//! holds for any Rust solver built on it.

mod common;

use common::example_bin;
use quip_solver_conformance::solve::{drive_solve, CaseOutcome};

/// A solver that advertises nothing optional passes the base cases, refuses a
/// linear schedule, and skips every limit case.
#[tokio::test]
async fn a_plain_solver_passes_the_base_cases() {
    let report = drive_solve(&example_bin("mock_sampler_miner").to_string_lossy()).await;
    assert!(report.is_conformant(), "{}", report.summary());
    for name in [
        "solve",
        "same-seed",
        "self-loop",
        "unknown-key",
        "linear-schedule",
        "anneal-time",
    ] {
        assert_eq!(
            report.outcome(name),
            Some(&CaseOutcome::Passed),
            "{name}\n{}",
            report.summary()
        );
    }
    for name in ["degree-limit-inside", "embed"] {
        assert!(
            matches!(report.outcome(name), Some(CaseOutcome::Skipped(_))),
            "{name}\n{}",
            report.summary()
        );
    }
}

/// Every optional case runs and passes against a solver that advertises all
/// of them.
#[tokio::test]
async fn a_solver_with_limits_passes_every_optional_case() {
    let report = drive_solve(&example_bin("mock_sampler_limits").to_string_lossy()).await;
    assert!(report.is_conformant(), "{}", report.summary());
    for name in [
        "linear-schedule",
        "degree-limit-inside",
        "degree-limit-outside",
        "field-limit-inside",
        "field-limit-outside",
        "resolution-limit-inside",
        "resolution-limit-outside",
    ] {
        assert_eq!(
            report.outcome(name),
            Some(&CaseOutcome::Passed),
            "{name}\n{}",
            report.summary()
        );
    }
}

/// The gate fails a solver whose sampler errors on every problem.
#[tokio::test]
async fn a_faulty_solver_fails_the_gate() {
    let report = drive_solve(&example_bin("mock_sampler_faulty").to_string_lossy()).await;
    assert!(!report.is_conformant(), "{}", report.summary());
    assert!(matches!(
        report.outcome("solve"),
        Some(CaseOutcome::Failed(_))
    ));
}
