//! Generates `quip_solver.h` from the `#[repr(C)]` surface in `src/lib.rs`, so
//! the header can never drift from the ABI it describes.
//!
//! The header lands beside the compiled library, in the Cargo profile directory
//! (`target/<profile>/quip_solver.h`), for two reasons. A build must not write
//! into `CARGO_MANIFEST_DIR`: that mutates the source tree and fails outright in
//! a read-only or vendored checkout. And `OUT_DIR` itself carries a content hash
//! in its name, so no non-Cargo build system — the release `Makefile`, the C++
//! example, a downstream `CMake` project — can find it without parsing Cargo's
//! JSON output. The profile directory is stable, always writable, and already
//! holds `libquip_solver_c.{so,a}`.
//!
//! Every failure here is fatal. A header that silently fails to regenerate is
//! worse than no header at all: the stale one still compiles, and a struct field
//! that moved becomes an out-of-bounds read at every call site.
#![expect(
    clippy::panic,
    clippy::expect_used,
    reason = "aborting the build is the only way a build script can report a failure, \
              and it is the point: a stale header must never survive a broken run"
)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let header = profile_dir(&out_dir).join("quip_solver.h");

    let bindings = match cbindgen::generate(&crate_dir) {
        Ok(bindings) => bindings,
        Err(e) => panic!(
            "cbindgen could not generate the C header from {}: {e}",
            crate_dir.display()
        ),
    };

    let mut generated = Vec::new();
    bindings.write(&mut generated);
    write_if_changed(&header, &generated);
}

/// Maps `OUT_DIR` back to the profile directory holding the compiled library.
///
/// Cargo lays `OUT_DIR` out as `<target>/<profile>/build/<pkg>-<hash>/out`, so
/// the profile directory is three levels up. The `build` component is checked
/// rather than assumed: if that layout ever changes, this must stop the build
/// instead of scattering a header somewhere nothing looks.
fn profile_dir(out_dir: &Path) -> PathBuf {
    let build_dir = out_dir.parent().and_then(Path::parent);
    let profile = build_dir.and_then(Path::parent);
    match (build_dir, profile) {
        (Some(build), Some(profile)) if build.file_name() == Some(OsStr::new("build")) => {
            profile.to_path_buf()
        }
        _ => panic!(
            "OUT_DIR {} is not the <target>/<profile>/build/<pkg>-<hash>/out path this build \
             script derives the header location from",
            out_dir.display()
        ),
    }
}

/// Writes `contents` to `path`, leaving the file alone when it already matches.
///
/// Skipping an identical write keeps the modification time stable, so a Rust
/// rebuild does not force every downstream C translation unit to recompile. The
/// write itself goes to a sibling file and is renamed into place, so an
/// interrupted build leaves either the old header or the new one, never half of
/// either.
fn write_if_changed(path: &Path, contents: &[u8]) {
    if std::fs::read(path).is_ok_and(|existing| existing == contents) {
        return;
    }
    let staged = path.with_extension("h.new");
    if let Err(e) = std::fs::write(&staged, contents) {
        panic!("cannot write the C header to {}: {e}", staged.display());
    }
    if let Err(e) = std::fs::rename(&staged, path) {
        panic!("cannot move the C header into {}: {e}", path.display());
    }
}
