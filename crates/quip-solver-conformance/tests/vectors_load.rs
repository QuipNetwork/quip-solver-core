//! The vectors must load from the crate's own manifest directory, so a
//! consumer in another repository reads the same bytes we do.

#[test]
fn adapt_cases_load_from_the_crate_not_the_workspace() {
    let cases = quip_solver_conformance::adapt_cases();
    assert_eq!(cases.len(), 28, "golden_adapt.json energy_to_difficulty");

    let params = quip_solver_conformance::adapt_params_cases();
    assert_eq!(params.len(), 10, "golden_adapt.json adapt_params_cpu_sa");
}

#[test]
fn golden_vectors_text_carries_every_section() {
    let text = quip_solver_conformance::GOLDEN_VECTORS;
    for section in [
        "chacha8",
        "derive_nonce",
        "diversity",
        "energy",
        "energy_rounding",
        "ising",
        "sentinel",
        "truncation",
        "version",
    ] {
        assert!(text.contains(section), "missing section {section}");
    }
}
