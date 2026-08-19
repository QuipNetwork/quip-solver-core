//! Generates `include/quip_solver.h` from the `#[repr(C)]` surface in
//! `src/lib.rs`, so the header can never drift from the ABI it describes.

use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = crate_dir.join("include").join("quip_solver.h");
    std::fs::create_dir_all(out.parent().expect("include dir")).expect("create include dir");

    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=cbindgen.toml");

    match cbindgen::generate(&crate_dir) {
        Ok(bindings) => {
            let _ = bindings.write_to_file(&out);
        }
        Err(e) => {
            // A missing header breaks the C and C++ consumers loudly at compile
            // time, so a warning here is enough; failing the Rust build would
            // stop the library itself from being produced.
            println!(
                "cargo:warning=cbindgen could not generate {}: {e}",
                out.display()
            );
        }
    }
}
