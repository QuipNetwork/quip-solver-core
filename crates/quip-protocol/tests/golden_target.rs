//! Proof target and integer diversity parity with the validator crate.
#![expect(
    clippy::indexing_slicing,
    reason = "golden JSON keys are fixed by the fixture"
)]

use quip_protocol::diversity::{diversity_milli, select_diverse};
use quip_protocol::target::{validate_proof_set, ProofStats, Target, TargetMiss};
use serde_json::Value;

#[expect(clippy::unwrap_used, reason = "fixture shape is fixed")]
fn golden() -> Value {
    serde_json::from_str(quip_solver_conformance::GOLDEN_TARGET).unwrap()
}

#[expect(clippy::unwrap_used, reason = "fixture spins are valid i8 values")]
fn spins(value: &Value) -> Vec<i8> {
    serde_json::from_value(value.clone()).unwrap()
}

#[test]
fn diversity_matches_chain() {
    let fixture = golden();
    let cases = fixture["diversity"].as_array().unwrap();
    assert!(!cases.is_empty());
    for (index, case) in cases.iter().enumerate() {
        let solutions: Vec<_> = case["solutions"]
            .as_array()
            .unwrap()
            .iter()
            .map(spins)
            .collect();
        let refs: Vec<_> = solutions.iter().map(Vec::as_slice).collect();
        assert_eq!(
            u64::from(diversity_milli(&refs)),
            case["diversity_milli"].as_u64().unwrap(),
            "diversity case {index}"
        );
    }
}

#[test]
fn selection_matches_chain() {
    let fixture = golden();
    let cases = fixture["select_diverse"].as_array().unwrap();
    assert!(!cases.is_empty());
    for (index, case) in cases.iter().enumerate() {
        let solutions: Vec<_> = case["solutions"]
            .as_array()
            .unwrap()
            .iter()
            .map(spins)
            .collect();
        let refs: Vec<_> = solutions.iter().map(Vec::as_slice).collect();
        let count = usize::try_from(case["target_count"].as_u64().unwrap()).unwrap();
        let expected: Vec<usize> = serde_json::from_value(case["indices"].clone()).unwrap();
        assert_eq!(
            select_diverse(&refs, count),
            expected,
            "selection case {index}"
        );
    }
}

#[test]
fn proof_checks_match_chain() {
    let fixture = golden();
    let cases = fixture["validate_proof"].as_array().unwrap();
    assert!(!cases.is_empty());
    for (index, case) in cases.iter().enumerate() {
        let set: Vec<_> = case["set"]
            .as_array()
            .unwrap()
            .iter()
            .map(|solution| {
                (
                    spins(&solution["spins"]),
                    solution["energy_milli"].as_i64().unwrap(),
                )
            })
            .collect();
        let refs: Vec<_> = set
            .iter()
            .map(|(spins, energy)| (spins.as_slice(), *energy))
            .collect();
        let target = Target {
            max_energy_milli: case["max_energy_milli"].as_i64().unwrap(),
            min_solutions: u32::try_from(case["min_solutions"].as_u64().unwrap()).unwrap(),
            min_diversity_milli: u32::try_from(case["min_diversity_milli"].as_u64().unwrap())
                .unwrap(),
            max_proof_solutions: 32,
        };
        let expected = match case["outcome"].as_str().unwrap() {
            "ok" => Ok(ProofStats {
                best_energy_milli: case["best_energy_milli"].as_i64().unwrap(),
                diversity_milli: u32::try_from(case["diversity_milli"].as_u64().unwrap()).unwrap(),
                valid_solution_count: u32::try_from(case["valid_solution_count"].as_u64().unwrap())
                    .unwrap(),
            }),
            "InsufficientEnergy" => Err(TargetMiss::InsufficientEnergy),
            "InsufficientSolutions" => Err(TargetMiss::InsufficientSolutions),
            "InsufficientDiversity" => Err(TargetMiss::InsufficientDiversity),
            outcome => unreachable!("unknown chain outcome {outcome}"),
        };
        assert_eq!(
            validate_proof_set(&refs, &target),
            expected,
            "proof case {index}"
        );
    }
}
