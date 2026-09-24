//! Generated tonic and prost stubs for `proto/quip/v1/miner.proto`.
//!
//! `src/generated/quip.v1.rs` is checked in rather than produced by a build
//! script, so a consumer of this crate needs no `protoc` on the build host.
//! `tests/stub_drift.rs` regenerates the stubs from the normative `.proto` and
//! fails when the checked-in copy is stale, which keeps the two in step.

#[expect(
    clippy::large_enum_variant,
    reason = "prost-generated CoordMsg stores the complete v2 Job by value"
)]
pub mod v1 {
    // An `include!` rather than a `mod` declaration, on purpose: rustfmt walks
    // `mod` declarations but not included files. This keeps the checked-in copy
    // byte-identical to what prost-build emits, which is what lets the drift
    // guard compare the two exactly instead of formatting both first.
    include!("generated/quip.v1.rs");
}
