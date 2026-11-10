//! Driver mode: one problem on stdin, one solution array on stdout, exit.

mod common;

use common::example_bin;
use std::io::Write as _;
use std::process::{Command, Output, Stdio};

const PROBLEM: &[u8] = br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],"num_reads":2,
                 "num_sweeps":8,"sweeps_per_beta":1,"beta_range":null,"seed":7}"#;

const ONE_READ: &[u8] = br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],"num_reads":1,
                 "num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#;

/// Run `<example> --solve` with `stdin` on its standard input. `label` names
/// the case in every failure.
#[expect(
    clippy::expect_used,
    reason = "test helper: each failure is asserted with its label first"
)]
fn run_solve(label: &str, example: &str, stdin: &[u8]) -> Output {
    let spawned = Command::new(example_bin(example))
        .arg("--solve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    assert!(spawned.is_ok(), "{label}: spawn --solve: {spawned:?}");
    let mut child = spawned.expect("spawned");
    let pipe = child.stdin.take();
    assert!(pipe.is_some(), "{label}: stdin");
    let written = pipe.expect("stdin").write_all(stdin);
    assert!(written.is_ok(), "{label}: write problem: {written:?}");
    let waited = child.wait_with_output();
    assert!(waited.is_ok(), "{label}: wait: {waited:?}");
    waited.expect("waited")
}

#[test]
fn solve_reads_a_problem_and_writes_solutions() {
    let out = run_solve("solve", "mock_sampler_miner", PROBLEM);

    assert!(out.status.success(), "exit {:?}", out.status.code());
    let sols: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(sols.len(), 2, "one solution per read");
    for s in &sols {
        assert_eq!(s["spins"].as_array().expect("spins").len(), 2);
        assert!(s["energy_milli"].as_i64().is_some());
    }
}

/// One row per `--solve` failure: a label for the failure message, the
/// example, the stdin, the exit code, the stderr needle (if any), and whether
/// stdout must be empty.
type SolveFailureRow = (
    &'static str,
    &'static str,
    &'static [u8],
    i32,
    Option<&'static str>,
    bool,
);

const SOLVE_FAILURES: &[SolveFailureRow] = &[
    (
        "malformed input exits config-invalid",
        "mock_sampler_miner",
        b"not json",
        64,
        Some("malformed problem JSON on stdin"),
        false,
    ),
    (
        "a sample error exits internal-fatal",
        "mock_sampler_faulty",
        ONE_READ,
        70,
        Some("solve failed"),
        false,
    ),
    // A model beyond the backend's read cap is capacity, exit 65: the caller
    // can route it to another solver, unlike malformed input (64) or a device
    // fault (70).
    (
        "a capacity refusal exits capacity",
        "mock_sampler_capped",
        ONE_READ,
        65,
        None,
        true,
    ),
];

#[test]
fn solve_mode_failures_map_to_exit_codes() {
    for &(label, example, stdin, code, needle, empty_stdout) in SOLVE_FAILURES {
        let out = run_solve(label, example, stdin);
        assert_eq!(out.status.code(), Some(code), "{label}");
        if let Some(needle) = needle {
            assert!(
                String::from_utf8_lossy(&out.stderr).contains(needle),
                "{label}: stderr should contain {needle:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        if empty_stdout {
            assert!(
                out.stdout.is_empty(),
                "{label}: a refused model writes no solutions"
            );
        }
    }
}

/// One row per startup case: a label for the failure message, the example, the
/// arguments, the environment changes (`Some` sets, `None` removes), and the
/// exit code.
type StartupRow = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [(&'static str, Option<&'static str>)],
    i32,
);

const STARTUP_FAILURES: &[StartupRow] = &[
    // Token is resolved before any connect. A missing QUIP_SESSION_TOKEN must
    // therefore exit 77 even when the coordinator URI cannot be reached. Exit
    // 70 here would mean the driver attempted the network first.
    (
        "a missing session token exits token-rejected before the network",
        "mock_sampler_miner",
        &["--quip-coordinator", "unix:///nonexistent"],
        &[("QUIP_SESSION_TOKEN", None)],
        77,
    ),
    (
        "--check on a working sampler exits clean",
        "mock_sampler_miner",
        &["--check"],
        &[],
        0,
    ),
    (
        "an open() failure on --check exits env-incompatible",
        "mock_sampler_unopenable",
        &["--check"],
        &[],
        69,
    ),
    // Session mode calls open() after the coordinator flag is present and
    // before the token/network path. A failing open must exit 69, not 77/70.
    (
        "an open() failure in session mode exits env-incompatible",
        "mock_sampler_unopenable",
        &["--quip-coordinator", "unix:///nonexistent"],
        &[("QUIP_SESSION_TOKEN", Some("test-token"))],
        69,
    ),
];

#[test]
fn startup_failures_map_to_exit_codes() {
    for &(label, example, args, env, code) in STARTUP_FAILURES {
        let mut command = Command::new(example_bin(example));
        let _ = command.stderr(Stdio::piped()).args(args);
        for &(key, value) in env {
            let _ = match value {
                Some(value) => command.env(key, value),
                None => command.env_remove(key),
            };
        }
        let output = command.output();
        assert!(output.is_ok(), "{label}: spawn: {output:?}");
        let out = output.expect("spawned");
        assert_eq!(
            out.status.code(),
            Some(code),
            "{label}: exit {:?}",
            out.status.code()
        );
    }
}
