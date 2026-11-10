//! End-to-end runs of the `quip-solver-drive --family` gate against real
//! example binaries. Each mode of the family binary is graded on its own run.

mod common;

use common::example_bin;
use quip_solver_conformance::families::{drive_miner_family, FamilyReport};
use quip_solver_conformance::solve::CaseOutcome;
use std::process::Command;

const UNIFORM: &str = "vectors/uniform-field@1";
const RESTRICTED: &str = "vectors/restricted-field@1";

const UNIFORM_NO_RESTRICTED: &str = "no reference params set gives code 10 (P_SPEC gives admitted, P_L2_F0 gives admitted, P_L3_F0 gives code 13)";
const RESTRICTED_NO_DEGREE: &str = "no reference params set gives code 13 (P_SPEC gives admitted, P_L2_F0 gives code 10, P_L3_F0 gives code 10)";
const LISTS_FAMILIES: &str = "the binary lists generated families";

/// An expected case: name, subject, and `Some(reason)` for a skip or `None`
/// for a pass.
type Expected = (&'static str, Option<&'static str>, Option<&'static str>);

/// The `target/<profile>` directory of the running test executable.
fn target_profile_dir() -> std::path::PathBuf {
    #[expect(
        clippy::expect_used,
        reason = "test helper: current_exe is always available under cargo test"
    )]
    let mut p = std::env::current_exe().expect("test exe path");
    let _ = p.pop(); // deps/
    let _ = p.pop(); // <profile>/
    p
}

/// The gate binary, built first when it is missing.
fn drive_bin() -> std::path::PathBuf {
    let path = target_profile_dir().join("quip-solver-drive");
    if !path.is_file() {
        #[expect(
            clippy::expect_used,
            reason = "test helper: cargo build failure is a setup error"
        )]
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "-p",
                "quip-solver-conformance",
                "--bin",
                "quip-solver-drive",
            ])
            .status()
            .expect("cargo build --bin");
        assert!(status.success(), "failed to build quip-solver-drive");
    }
    path
}

fn unique_socket(tag: &str) -> String {
    let nanos = {
        #[expect(
            clippy::unwrap_used,
            reason = "test helper: system clock is after UNIX_EPOCH"
        )]
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    };
    format!("/tmp/quip-core-{tag}-{}-{nanos}.sock", std::process::id())
}

/// The per-revision case names the gate runs for each revision, in order.
const REVISION_CASES: [&str; 10] = [
    "supported-revision",
    "revision-plus-one",
    "params-malformed",
    "params-digest-mismatch",
    "params-out-of-bounds",
    "params-restricted",
    "topology-order-mismatch",
    "degree-bound-too-large",
    "stale-lease",
    "deadline-expired",
];

/// The explicit-job case names the gate runs for each accepted revision, in
/// order, after every lease case.
const EXPLICIT_CASES: [&str; 5] = [
    "explicit-supported",
    "explicit-digest-mismatch",
    "explicit-metadata-invalid",
    "explicit-params-out-of-bounds",
    "explicit-topology-hash-form",
];

/// The pinned ordered case list of `mock_sampler_family_lease`.
fn family_binary_cases() -> Vec<Expected> {
    let mut cases: Vec<Expected> = vec![
        ("hello-protocol-version-2", None, None),
        ("generators-list-family-iff-generated", None, None),
        ("session-setup", None, None),
        (
            "no-generated-families-rejects-family-lease",
            None,
            Some(LISTS_FAMILIES),
        ),
        ("unknown-family", None, None),
    ];
    for subject in [UNIFORM, RESTRICTED] {
        for name in REVISION_CASES {
            let skip = match (subject, name) {
                (UNIFORM, "params-restricted") => Some(UNIFORM_NO_RESTRICTED),
                (RESTRICTED, "degree-bound-too-large") => Some(RESTRICTED_NO_DEGREE),
                _ => None,
            };
            cases.push((name, Some(subject), skip));
        }
    }
    for subject in [UNIFORM, RESTRICTED] {
        for name in EXPLICIT_CASES {
            cases.push((name, Some(subject), None));
        }
    }
    cases.push(("credit-ledger-balanced", None, None));
    cases.push(("clean-exit", None, None));
    cases
}

/// One case as owned text: name, subject, and the skip reason or failure.
type Got = (String, Option<String>, Option<String>);

/// Assert the report holds exactly `expected`, in order, with the pinned
/// skip reasons. A failed case shows as `FAILED: why`.
fn assert_cases(report: &FamilyReport, expected: &[Expected]) {
    let got: Vec<Got> = report
        .cases
        .iter()
        .map(|case| {
            let note = match &case.outcome {
                CaseOutcome::Passed => None,
                CaseOutcome::Skipped(why) => Some(why.clone()),
                CaseOutcome::Failed(why) => Some(format!("FAILED: {why}")),
            };
            (case.name.to_owned(), case.subject.clone(), note)
        })
        .collect();
    let want: Vec<Got> = expected
        .iter()
        .map(|(name, subject, skip)| {
            (
                (*name).to_owned(),
                subject.map(str::to_owned),
                skip.map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(got, want, "{}", report.summary());
}

#[tokio::test]
async fn the_default_path_passes_the_family_gate() {
    let bin = example_bin("mock_sampler_family_lease")
        .to_string_lossy()
        .into_owned();
    let socket = unique_socket("fam-default");
    let report =
        drive_miner_family(&bin, &["--mode", "default"], &format!("unix://{socket}")).await;
    assert!(report.is_conformant(), "{}", report.summary());
    assert_cases(&report, &family_binary_cases());
}

#[tokio::test]
async fn the_local_path_passes_the_family_gate() {
    let bin = example_bin("mock_sampler_family_lease")
        .to_string_lossy()
        .into_owned();
    let socket = unique_socket("fam-local");
    let report = drive_miner_family(&bin, &["--mode", "local"], &format!("unix://{socket}")).await;
    assert!(report.is_conformant(), "{}", report.summary());
    assert_cases(&report, &family_binary_cases());
}

#[tokio::test]
async fn the_aware_path_passes_the_family_gate() {
    let bin = example_bin("mock_sampler_family_lease")
        .to_string_lossy()
        .into_owned();
    let socket = unique_socket("fam-aware");
    let report = drive_miner_family(&bin, &["--mode", "aware"], &format!("unix://{socket}")).await;
    assert!(report.is_conformant(), "{}", report.summary());
    assert_cases(&report, &family_binary_cases());
}

/// An ice binary lists `quip/ice-glass@1` (z12) and `quip/twisted-ice-dimer@1`
/// (z22), so the gate sends a different topology to each subject.
#[tokio::test]
async fn an_ice_binary_passes_the_family_gate_on_the_topology_of_each_revision() {
    let bin = example_bin("mock_sampler_family_ice")
        .to_string_lossy()
        .into_owned();
    let socket = unique_socket("fam-ice");
    let report = drive_miner_family(&bin, &[], &format!("unix://{socket}")).await;
    assert!(report.is_conformant(), "{}", report.summary());
    for subject in ["quip/ice-glass@1", "quip/twisted-ice-dimer@1"] {
        assert_eq!(
            report.outcome("supported-revision", Some(subject)),
            Some(&CaseOutcome::Passed),
            "{}",
            report.summary()
        );
        for name in EXPLICIT_CASES {
            assert_eq!(
                report.outcome(name, Some(subject)),
                Some(&CaseOutcome::Passed),
                "{name} {subject}\n{}",
                report.summary()
            );
        }
    }
}

#[tokio::test]
async fn a_generic_binary_is_not_asked_to_generate() {
    let bin = example_bin("mock_sampler_lease")
        .to_string_lossy()
        .into_owned();
    let socket = unique_socket("fam-generic");
    let report = drive_miner_family(&bin, &[], &format!("unix://{socket}")).await;
    assert!(report.is_conformant(), "{}", report.summary());
    assert_cases(&report, &generic_binary_cases());
}

#[test]
fn the_family_gate_cli_reports_and_exits_zero() {
    let bin = example_bin("mock_sampler_family_lease")
        .to_string_lossy()
        .into_owned();
    let socket = unique_socket("fam-cli");
    let out = Command::new(drive_bin())
        .args(["--family", &bin, &format!("unix://{socket}")])
        .output()
        .expect("run quip-solver-drive");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("[pass] supported-revision vectors/uniform-field@1"),
        "{stdout}"
    );
}

/// The pinned ordered case list of `mock_sampler_lease`, a generic binary.
fn generic_binary_cases() -> Vec<Expected> {
    vec![
        ("hello-protocol-version-2", None, None),
        ("generators-list-family-iff-generated", None, None),
        ("session-setup", None, None),
        ("no-generated-families-rejects-family-lease", None, None),
        (
            "unknown-family",
            None,
            Some("the binary lists no generated families"),
        ),
        ("credit-ledger-balanced", None, None),
        ("clean-exit", None, None),
    ]
}
