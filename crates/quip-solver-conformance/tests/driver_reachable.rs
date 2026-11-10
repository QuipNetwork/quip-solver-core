//! The driver must be reachable from this crate, because that is the single
//! dev-dependency a solver repository takes.

#[test]
fn public_surface_is_exported() {
    use quip_solver_conformance::driver::{DriverReport, LeaseOutcome, Terminal};
    use quip_solver_conformance::families::layered::{LAYERED, RESTRICTED_V1, UNIFORM_V1};
    use quip_solver_conformance::families::registry::{
        archived_among, parse_registry, unregistered_quip_ids, RegistryEntry, Status,
    };
    use quip_solver_conformance::families::{
        check_case, compare, drive_miner_family, export_instances, first_diff, parse_vector_cases,
        reference_revision, CaseMismatch, Component, Expected, ExpectedSalt, FamilyCase,
        FamilyReport, FirstDiff, VectorCase,
    };
    use quip_solver_conformance::solve::CaseOutcome;

    // The drive functions. A compile-time check: naming the paths proves they
    // resolve. The values are never called. Do NOT annotate these as
    // `fn(&str, &str) -> _`: both are `async fn`, so the return is an opaque
    // future that borrows both arguments, and it does not coerce to a fn pointer
    // (E0308).
    let _ = quip_solver_conformance::driver::drive_miner;
    let _ = quip_solver_conformance::driver::drive_miner_bad_welcome;
    let _ = quip_solver_conformance::driver::drive_miner_one_job;
    let _ = quip_solver_conformance::driver::drive_miner_close_after_welcome;
    let _ = quip_solver_conformance::driver::drive_miner_close_before_welcome;

    // The verdict surface. A solver repository grades itself with these, so they
    // are as much of the contract as the drive functions are.
    let _ = DriverReport::is_conformant;
    let _ = DriverReport::lease_conformant;
    let _ = DriverReport::bad_welcome_conformant;
    let _ = DriverReport::close_after_welcome_conformant;
    let _ = DriverReport::close_before_welcome_conformant;
    let _ = DriverReport::credit_ledger_balanced;
    let _ = DriverReport::energies_rescore_clean;
    let _ = DriverReport::live_cancel_conformant;
    let _ = DriverReport::summary;
    let _ = LeaseOutcome {
        results: 4,
        results_verified: 4,
        lease_done: Some((4, -1000)),
        credit_refunded: true,
    };
    let _ = Terminal::Open;

    // The wire fixture.
    let wire = quip_solver_conformance::golden_wire();
    assert!(
        wire.messages.contains_key("hello"),
        "golden_wire.json must carry the Hello encoding"
    );
    assert!(
        wire.enums.contains_key("RejectReason"),
        "golden_wire.json must carry the RejectReason numbers"
    );

    // The layered construction.
    assert_eq!(UNIFORM_V1.family_id, "vectors/uniform-field");
    assert_eq!(RESTRICTED_V1.family_id, "vectors/restricted-field");
    assert_eq!(LAYERED.labels, ["salt", "couplings"]);

    // The family checker.
    let _ = (
        export_instances,
        parse_vector_cases,
        first_diff,
        compare,
        check_case,
    );
    let _: Option<(
        CaseMismatch,
        Component,
        Expected,
        ExpectedSalt,
        FirstDiff,
        VectorCase,
    )> = None;

    // The family gate surface.
    let _ = drive_miner_family;
    let _ = FamilyReport::is_conformant;
    let _ = FamilyReport::outcome;
    let _ = FamilyReport::summary;
    let _ = FamilyCase {
        name: "revision-plus-one",
        subject: None,
        outcome: CaseOutcome::Passed,
    };
    assert!(reference_revision("vectors/uniform-field", 1).is_some());
    assert!(reference_revision("vectors/uniform-field", 2).is_none());

    // The family registry.
    let registry = parse_registry(quip_solver_conformance::FAMILY_REGISTRY);
    let _ = archived_among;
    assert_eq!(unregistered_quip_ids(&[], ["quip/ice"]), ["quip/ice"]);
    let _: Option<(RegistryEntry, Status)> = None;
    assert!(registry.is_ok_and(|entries| !entries.is_empty()));
}
