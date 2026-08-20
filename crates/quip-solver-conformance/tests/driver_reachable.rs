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
    let _ = quip_solver_conformance::driver::drive_miner_one_job;
    let _ = quip_solver_conformance::driver::drive_miner_close_after_welcome;
    let _ = quip_solver_conformance::driver::drive_miner_close_before_welcome;
}

#[test]
fn the_verdict_surface_is_exported_too() {
    // A solver repository grades itself with these, so they are as much of the
    // contract as the drive functions are.
    use quip_solver_conformance::driver::{DriverReport, Terminal};

    let _ = DriverReport::is_conformant;
    let _ = DriverReport::bad_welcome_conformant;
    let _ = DriverReport::close_after_welcome_conformant;
    let _ = DriverReport::close_before_welcome_conformant;
    let _ = DriverReport::credit_ledger_balanced;
    let _ = DriverReport::energies_rescore_clean;
    let _ = DriverReport::live_cancel_conformant;
    let _ = DriverReport::summary;
    assert_eq!(Terminal::default(), Terminal::Open);
}

#[test]
fn the_wire_fixture_is_reachable() {
    let wire = quip_solver_conformance::golden_wire();
    assert!(
        wire.messages.contains_key("hello"),
        "golden_wire.json must carry the Hello encoding"
    );
    assert!(
        wire.enums.contains_key("RejectReason"),
        "golden_wire.json must carry the RejectReason numbers"
    );
}
