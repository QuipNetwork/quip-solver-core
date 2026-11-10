//! Locate the example binaries of this package from inside an integration
//! test.
//!
//! `cargo test` builds every example of the package under test before it runs
//! the tests, so the binary is normally on disk already. A nested
//! `cargo build` would resolve features for a single package and recompile
//! dependencies the workspace build already compiled. The helper builds an
//! example only when its binary is missing, for example when one test file is
//! run alone from a clean target directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// The `target/<profile>` directory of the running test executable.
#[expect(
    clippy::expect_used,
    reason = "test helper: current_exe is always available under cargo test"
)]
pub(crate) fn profile_dir() -> PathBuf {
    let mut path = std::env::current_exe().expect("test exe path");
    let _ = path.pop(); // deps/
    let _ = path.pop(); // <profile>/
    path
}

/// The cargo profile name that produced `profile_dir`, for a nested build
/// that must land in the same directory. Cargo writes the `dev` profile to
/// `debug/`; every other profile directory carries the profile's own name.
pub(crate) fn profile_name(profile_dir: &Path) -> String {
    match profile_dir.file_name().and_then(|name| name.to_str()) {
        Some("debug") | None => "dev".to_owned(),
        Some(name) => name.to_owned(),
    }
}

/// The path of the named example binary, built first when it is missing.
///
/// A test process builds each name at most once.
#[expect(
    clippy::expect_used,
    reason = "test helper: a missing example is a setup error"
)]
pub(crate) fn example_bin(name: &str) -> PathBuf {
    static CACHE: OnceLock<Mutex<HashMap<String, PathBuf>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .expect("example cache lock");
    if let Some(path) = cache.get(name) {
        return path.clone();
    }
    let profile = profile_dir();
    let path = profile.join("examples").join(name);
    if !path.is_file() {
        let status = Command::new(env!("CARGO"))
            .args(["build", "--example", name, "-p", "quip-solver-core"])
            .args(["--profile", &profile_name(&profile)])
            .status()
            .expect("cargo build --example");
        assert!(status.success(), "failed to build example {name}");
        assert!(
            path.is_file(),
            "example {name} not found at {}",
            path.display()
        );
    }
    let _ = cache.insert(name.to_owned(), path.clone());
    path
}
