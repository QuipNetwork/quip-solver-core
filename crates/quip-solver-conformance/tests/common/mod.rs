//! Helpers shared by the `--solve` gate tests.

use std::path::PathBuf;
use std::process::Command;

/// Build and locate the example named `name`.
#[expect(
    clippy::expect_used,
    reason = "test helper: current_exe and the example build are setup steps"
)]
pub(crate) fn example(name: &str) -> PathBuf {
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
