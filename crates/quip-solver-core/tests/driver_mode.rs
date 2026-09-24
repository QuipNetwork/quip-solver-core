//! Driver mode: one problem on stdin, one solution array on stdout, exit.

use std::io::Write as _;
use std::process::{Command, Stdio};

fn example_bin(name: &str) -> String {
    #[expect(
        clippy::expect_used,
        reason = "test helper: cargo build failure is a setup error"
    )]
    let status = Command::new(env!("CARGO"))
        .args(["build", "--example", name, "-p", "quip-solver-core"])
        .status()
        .expect("cargo build --example");
    assert!(status.success(), "failed to build example {name}");
    #[expect(
        clippy::expect_used,
        reason = "test helper: current_exe is always available under cargo test"
    )]
    let mut p = std::env::current_exe().expect("test exe path");
    let _ = p.pop();
    let _ = p.pop();
    p.push("examples");
    p.push(name);
    p.to_string_lossy().into_owned()
}

#[test]
fn solve_reads_a_problem_and_writes_solutions() {
    let mut child = Command::new(example_bin("mock_sampler_miner"))
        .arg("--solve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn --solve");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(
            br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],"num_reads":2,
                 "num_sweeps":8,"sweeps_per_beta":1,"beta_range":null,"seed":7}"#,
        )
        .expect("write problem");
    let out = child.wait_with_output().expect("wait");

    assert!(out.status.success(), "exit {:?}", out.status.code());
    let sols: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(sols.len(), 2, "one solution per read");
    for s in &sols {
        assert_eq!(s["spins"].as_array().expect("spins").len(), 2);
        assert!(s["energy_milli"].as_i64().is_some());
    }
}

#[test]
fn malformed_input_exits_config_invalid() {
    let mut child = Command::new(example_bin("mock_sampler_miner"))
        .arg("--solve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn --solve");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"not json")
        .expect("write");
    let out = child.wait_with_output().expect("wait");

    assert_eq!(out.status.code(), Some(64), "malformed input must exit 64");
    assert!(String::from_utf8_lossy(&out.stderr).contains("malformed problem JSON on stdin"));
}

#[test]
fn sample_error_exits_internal_fatal() {
    let mut child = Command::new(example_bin("mock_sampler_faulty"))
        .arg("--solve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn --solve");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(
            br#"{"h":[1.0,-1.0],"j":[1.0],"edges":[[0,1]],"num_reads":1,
                 "num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":1}"#,
        )
        .expect("write problem");
    let out = child.wait_with_output().expect("wait");

    assert_eq!(out.status.code(), Some(70), "SampleError must exit 70");
    assert!(String::from_utf8_lossy(&out.stderr).contains("solve failed"));
}

#[test]
fn missing_session_token_exits_token_rejected_before_network() {
    // Token is resolved before any connect. A missing QUIP_SESSION_TOKEN must
    // therefore exit 77 even when the coordinator URI cannot be reached. Exit
    // 70 here would mean the driver attempted the network first.
    let out = Command::new(example_bin("mock_sampler_miner"))
        .stderr(Stdio::piped())
        .arg("--quip-coordinator")
        .arg("unix:///nonexistent")
        .env_remove("QUIP_SESSION_TOKEN")
        .output()
        .expect("spawn missing-token session");
    assert_eq!(
        out.status.code(),
        Some(77),
        "missing QUIP_SESSION_TOKEN must exit 77, not 70: {:?}",
        out.status.code()
    );
}

#[test]
fn check_on_working_sampler_exits_clean() {
    let out = Command::new(example_bin("mock_sampler_miner"))
        .stderr(Stdio::piped())
        .arg("--check")
        .output()
        .expect("spawn --check");
    assert_eq!(
        out.status.code(),
        Some(0),
        "--check on a working sampler must exit 0: {:?}",
        out.status.code()
    );
}

#[test]
fn unopenable_sampler_check_exits_env_incompatible() {
    let out = Command::new(example_bin("mock_sampler_unopenable"))
        .stderr(Stdio::piped())
        .arg("--check")
        .output()
        .expect("spawn unopenable --check");
    assert_eq!(
        out.status.code(),
        Some(69),
        "open() failure on --check must exit 69: {:?}",
        out.status.code()
    );
}

#[test]
fn unopenable_sampler_session_exits_env_incompatible() {
    // Session mode calls open() after the coordinator flag is present and
    // before the token/network path. A failing open must exit 69, not 77/70.
    let out = Command::new(example_bin("mock_sampler_unopenable"))
        .stderr(Stdio::piped())
        .arg("--quip-coordinator")
        .arg("unix:///nonexistent")
        .env("QUIP_SESSION_TOKEN", "test-token")
        .output()
        .expect("spawn unopenable session");
    assert_eq!(
        out.status.code(),
        Some(69),
        "open() failure in session mode must exit 69: {:?}",
        out.status.code()
    );
}
