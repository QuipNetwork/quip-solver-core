//! `--capabilities` must emit the typed Capabilities message, and it must keep
//! the seven keys the current readers already parse.

mod common;

use common::example_bin;
use std::process::Command;

#[test]
fn capabilities_flag_emits_the_typed_message() {
    let out = Command::new(example_bin("mock_sampler_miner"))
        .arg("--capabilities")
        .output()
        .expect("run --capabilities");
    assert!(out.status.success());

    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(v.get("backend"), Some(&serde_json::json!("mock")));
    assert_eq!(v.get("algorithm"), Some(&serde_json::json!("sa")));
    assert_eq!(
        v.get("supportedKinds"),
        Some(&serde_json::json!(["ISING_SAMPLE", "ISING_GENERATE"]))
    );
    assert!(v
        .get("maxNodes")
        .and_then(serde_json::Value::as_u64)
        .is_some());
    assert!(v
        .get("maxEdges")
        .and_then(serde_json::Value::as_u64)
        .is_some());
    assert_eq!(v.get("protocolVersion"), Some(&serde_json::json!(2)));
    assert_eq!(
        v.get("encodings"),
        Some(&serde_json::json!([
            "COEFFICIENT_ENCODING_I32",
            "COEFFICIENT_ENCODING_F64"
        ]))
    );
    assert_eq!(
        v.get("generators"),
        Some(&serde_json::json!([
            "GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1"
        ]))
    );
    assert!(v
        .get("streamWidth")
        .and_then(serde_json::Value::as_u64)
        .is_some());
    for key in ["solverId", "generatedFamilies", "acceptedFamilies"] {
        assert!(
            v.get(key).is_none(),
            "{key} is absent from a generic solver"
        );
    }
}

/// The exact `--capabilities` line of a solver with no family declaration.
#[test]
fn generic_capabilities_output_is_unchanged() {
    let out = Command::new(example_bin("mock_sampler_miner"))
        .arg("--capabilities")
        .output()
        .expect("run --capabilities");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        concat!(
            "{\"supportedKinds\":[\"ISING_SAMPLE\",\"ISING_GENERATE\"],\"maxNodes\":100000,\"maxEdges\":1000000,\"features\":[],\"protocolVersion\":2,\"streamWidth\":1,\"encodings\":[\"COEFFICIENT_ENCODING_I32\",\"COEFFICIENT_ENCODING_F64\"],\"generators\":[\"GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1\"],\"backend\":\"mock\",\"algorithm\":\"sa\"}",
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
