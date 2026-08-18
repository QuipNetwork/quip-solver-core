//! The driver must be reachable from this crate, because that is the single
//! dev-dependency a solver repository takes.

#[test]
fn drive_miner_is_exported_from_the_conformance_crate() {
    // A compile-time check: naming the paths proves they resolve. The values
    // are never called. Do NOT annotate these as `fn(&str, &str) -> _`: both
    // are `async fn`, so the return is an opaque future that borrows both
    // arguments, and it does not coerce to a fn pointer (E0308).
    let _ = quip_solver_conformance::driver::drive_miner;
    let _ = quip_solver_conformance::driver::drive_miner_bad_welcome;
}
