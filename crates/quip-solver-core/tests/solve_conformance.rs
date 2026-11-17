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
    // A solver that advertises no optional feature passes the base cases,
    // refuses a linear schedule, and skips every limit case. It lists
    // IQM_SAMPLE, so it solves the IQM form.
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
            "option-undeclared",
            "iqm-solve",
            "iqm-bounds-mismatch",
        ],
        &[
            "degree-limit-inside",
            "embed",
            "option-valid",
            "option-wrong-type",
        ],
        &[],
    ),
    // Every optional case runs and passes against a solver that advertises all
    // of them. It does not list IQM_SAMPLE, so it refuses the IQM form.
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
            "option-valid",
            "option-undeclared",
            "option-wrong-type",
            "iqm-unsupported",
        ],
        &["iqm-solve", "iqm-bounds-mismatch"],
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
