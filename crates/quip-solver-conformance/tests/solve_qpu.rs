//! The `--solve` gate's QPU cases, run against the crate's stand-in annealer.

use quip_solver_conformance::solve::{drive_solve, CaseOutcome};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The stand-in annealer binary. `cargo test` builds the examples of the
/// package under test first, so a nested build runs only when it is missing.
/// The path is resolved once per test process.
#[expect(
    clippy::expect_used,
    reason = "test helper: current_exe and the example build are setup steps"
)]
fn mock_qpu_solver() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let mut profile = std::env::current_exe().expect("test exe path");
        let _ = profile.pop(); // deps/
        let _ = profile.pop(); // <profile>/
        let path = profile.join("examples").join("mock_qpu_solver");
        if !path.is_file() {
            // Cargo writes the `dev` profile to `debug/`; every other
            // profile directory carries the profile's own name.
            let profile_name = match profile.file_name().and_then(|n| n.to_str()) {
                Some("debug") | None => "dev",
                Some(name) => name,
            };
            let status = Command::new(env!("CARGO"))
                .args([
                    "build",
                    "--example",
                    "mock_qpu_solver",
                    "-p",
                    "quip-solver-conformance",
                    "--profile",
                    profile_name,
                ])
                .status()
                .expect("cargo build --example");
            assert!(status.success(), "failed to build mock_qpu_solver");
            assert!(
                path.is_file(),
                "mock_qpu_solver not found at {}",
                path.display()
            );
        }
        path
    })
}

/// An annealer that lists `anneal-time` and `embed` is conformant, and both
/// cases run and pass rather than skip.
#[tokio::test]
async fn an_annealer_passes_the_anneal_time_and_embed_cases() {
    let report = drive_solve(&mock_qpu_solver().to_string_lossy()).await;
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
