//! Guards the checked-in `src/generated/quip.v1.rs` against drift from `proto/quip/v1/miner.proto`.
//!
//! This crate ships generated code instead of running `tonic-build` from a
//! build script, so no build step forces the stubs and the normative `.proto`
//! to agree. This test is what forces it. It mirrors the Python stub drift
//! guard in `.gitlab-ci.yml`.

use std::path::{Path, PathBuf};

#[test]
fn checked_in_stubs_match_the_normative_proto() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let proto_root = crate_dir.join("../../proto");
    let proto = proto_root.join("quip/v1/miner.proto");

    assert!(
        proto.is_file(),
        "{} is missing. This guard runs against the repository tree; the \
         published .crate ships only the generated stubs and does not \
         carry the .proto.",
        proto.display()
    );

    let out_dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("stub-drift");
    std::fs::create_dir_all(&out_dir).expect("create the regeneration output directory");

    tonic_build::configure()
        .out_dir(&out_dir)
        .compile_protos(&[&proto], &[&proto_root])
        .expect("regenerate the stubs; this needs protoc on PATH");

    let regenerated_path = out_dir.join("quip.v1.rs");
    let regenerated =
        std::fs::read_to_string(&regenerated_path).expect("read the regenerated stubs");
    let committed_path = crate_dir.join("src/generated/quip.v1.rs");
    let committed = std::fs::read_to_string(&committed_path).expect("read the checked-in stubs");

    if regenerated != committed {
        let first_difference = regenerated
            .lines()
            .zip(committed.lines())
            .position(|(new, old)| new != old)
            .map_or_else(
                || "the files differ in length".to_string(),
                |index| format!("first difference at line {}", index + 1),
            );
        panic!(
            "crates/quip-proto/src/generated/quip.v1.rs is stale against proto/quip/v1/miner.proto \
             ({first_difference}).\nRefresh it with:\n    cp {} {}",
            regenerated_path.display(),
            committed_path.display()
        );
    }
}
