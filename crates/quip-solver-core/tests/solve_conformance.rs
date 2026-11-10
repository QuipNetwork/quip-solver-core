//! The `--solve` gate from quip-solver-conformance, run against the SDK's own
//! test-support miners. A green run here means the SDK side of SPEC section 2
//! holds for any Rust solver built on it.

mod common;

use common::example_bin;
use quip_solver_conformance::solve::{drive_solve, CaseOutcome};

/// One row per example: the binary, whether the gate passes it, and the cases
/// that must pass, be skipped, or fail.
type GateRow = (
    &'static str,
    bool,
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
);

const GATE_ROWS: &[GateRow] = &[
    // A solver that advertises nothing optional passes the base cases, refuses
    // a linear schedule, and skips every limit case.
    (
        "mock_sampler_miner",
        true,
        &[
            "solve",
            "same-seed",
            "self-loop",
            "unknown-key",
            "linear-schedule",
            "anneal-time",
        ],
        &["degree-limit-inside", "embed"],
        &[],
    ),
    // Every optional case runs and passes against a solver that advertises all
    // of them.
    (
        "mock_sampler_limits",
        true,
        &[
            "linear-schedule",
            "degree-limit-inside",
            "degree-limit-outside",
            "field-limit-inside",
            "field-limit-outside",
            "resolution-limit-inside",
            "resolution-limit-outside",
        ],
        &[],
        &[],
    ),
    // The gate fails a solver whose sampler errors on every problem.
    ("mock_sampler_faulty", false, &[], &[], &["solve"]),
];

#[tokio::test]
async fn the_solve_gate_grades_each_example() {
    for &(example, conformant, passed, skipped, failed) in GATE_ROWS {
        let report = drive_solve(&example_bin(example).to_string_lossy()).await;
        assert_eq!(
            report.is_conformant(),
            conformant,
            "{example}\n{}",
            report.summary()
        );
        for name in passed {
            assert_eq!(
                report.outcome(name),
                Some(&CaseOutcome::Passed),
                "{example}: {name}\n{}",
                report.summary()
            );
        }
        for name in skipped {
            assert!(
                matches!(report.outcome(name), Some(CaseOutcome::Skipped(_))),
                "{example}: {name}\n{}",
                report.summary()
            );
        }
        for name in failed {
            assert!(
                matches!(report.outcome(name), Some(CaseOutcome::Failed(_))),
                "{example}: {name}\n{}",
                report.summary()
            );
        }
    }
}
