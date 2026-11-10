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
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

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
    let mut path = std::env::current_exe().expect("test exe path");
    let _ = path.pop(); // deps/
    let _ = path.pop(); // <profile>/
    path.push("examples");
    path.push(name);
    if !path.is_file() {
        let status = Command::new(env!("CARGO"))
            .args(["build", "--example", name, "-p", "quip-solver-core"])
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
