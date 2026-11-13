//! `--capabilities` must emit the typed Capabilities message, and it must keep
//! the seven keys the current readers already parse.

mod common;

use common::example_bin;
use std::process::Command;

/// The exact `--capabilities` line of a solver with no family declaration.
/// The default session path generates every registered revision, so the
/// line lists them and the family generator (FAM-58).
#[test]
fn generic_capabilities_output() {
    let out = Command::new(example_bin("mock_sampler_miner"))
        .arg("--capabilities")
        .output()
        .expect("run --capabilities");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        concat!(
            "{\"supportedKinds\":[\"ISING_SAMPLE\",\"ISING_GENERATE\"],\"maxNodes\":100000,\"maxEdges\":1000000,\"features\":[],\"protocolVersion\":2,\"streamWidth\":1,\"encodings\":[\"COEFFICIENT_ENCODING_I32\",\"COEFFICIENT_ENCODING_F64\"],\"generators\":[\"GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1\",\"GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1\"],\"backend\":\"mock\",\"algorithm\":\"sa\",",
            "\"generatedFamilies\":[{\"familyId\":\"quip/charged-ice\",\"revisions\":[1]},{\"familyId\":\"quip/defects-static\",\"revisions\":[1]},{\"familyId\":\"quip/flow-static\",\"revisions\":[1]},{\"familyId\":\"quip/ice-fields\",\"revisions\":[1]},{\"familyId\":\"quip/ice-glass\",\"revisions\":[1]},{\"familyId\":\"quip/polariton-static\",\"revisions\":[1]},{\"familyId\":\"quip/twisted-ice-dimer\",\"revisions\":[1]},{\"familyId\":\"quip/two-commodity-routing\",\"revisions\":[1]}]}",
            "\n"
        )
    );
}

/// `--capabilities` never opens the device: the unopenable example answers.
#[test]
fn capabilities_answer_without_opening_the_device() {
    let out = Command::new(example_bin("mock_sampler_unopenable"))
        .arg("--capabilities")
        .output()
        .expect("run --capabilities");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(v.is_object());
}
