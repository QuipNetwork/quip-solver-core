//! Golden energy, diversity, rounding, and sentinel vectors vs
//! `conformance/golden_vectors.json`.
#![expect(
    clippy::indexing_slicing,
    reason = "golden JSON keys are fixed by the fixture"
)]
#![expect(
    clippy::cast_possible_truncation,
    reason = "golden spins are ±1 and edge indices are in-range by fixture contract"
)]
#![expect(
    clippy::cast_precision_loss,
    reason = "milli values cast to f64 to rebuild the wire coefficients callers pass"
)]

use quip_protocol::scoring::{
    energy_from_milli, energy_milli, set_diversity, ENERGY_MILLI_NON_FINITE,
};
use serde_json::Value;

#[expect(
    clippy::unwrap_used,
    reason = "integration-test helper; missing golden fixture should panic"
)]
fn golden() -> Value {
    serde_json::from_str(quip_solver_conformance::GOLDEN_VECTORS).unwrap()
}

#[expect(
    clippy::unwrap_used,
    reason = "integration-test helper; fixture shape is fixed"
)]
fn spins_of(case: &Value) -> Vec<i8> {
    case["spins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap() as i8)
        .collect()
}

#[expect(
    clippy::unwrap_used,
    reason = "integration-test helper; fixture shape is fixed"
)]
fn edges_of(case: &Value) -> Vec<(usize, usize)> {
    case["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e[0].as_u64().unwrap() as usize,
                e[1].as_u64().unwrap() as usize,
            )
        })
        .collect()
}

/// Rebuild the `f64` coefficients a caller passes, exactly as every binding does:
/// the wire carries `i32` milli, and the caller divides by 1000.
#[expect(
    clippy::unwrap_used,
    reason = "integration-test helper; fixture shape is fixed"
)]
fn coefficients_of(case: &Value, key: &str) -> Vec<f64> {
    case[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap() as f64 / 1000.0)
        .collect()
}

/// The wire's `i32` milli coefficients, as `IsingGraph` holds them.
#[expect(
    clippy::unwrap_used,
    reason = "integration-test helper; fixture shape is fixed"
)]
fn milli_of(case: &Value, key: &str) -> Vec<i32> {
    case[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| i32::try_from(v.as_i64().unwrap()).unwrap())
        .collect()
}

#[test]
fn energy_matches_golden() {
    for (index, case) in golden()["energy"].as_array().unwrap().iter().enumerate() {
        let spins = spins_of(case);
        let edges = edges_of(case);
        let want = case["energy_milli"].as_i64().unwrap();
        let h = coefficients_of(case, "h_milli");
        let j = coefficients_of(case, "j_milli");
        assert_eq!(
            energy_milli(&spins, &h, &j, &edges),
            want,
            "energy case {index}"
        );
        let h_milli = milli_of(case, "h_milli");
        let j_milli = milli_of(case, "j_milli");
        assert_eq!(
            energy_from_milli(&spins, &h_milli, &j_milli, &edges),
            want,
            "energy case {index}, integer path"
        );
    }
}

#[test]
fn diversity_matches_golden() {
    for case in golden()["diversity"].as_array().unwrap() {
        let sols: Vec<Vec<i8>> = case["solutions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                s.as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_i64().unwrap() as i8)
                    .collect()
            })
            .collect();
        assert!((set_diversity(&sols) - case["diversity"].as_f64().unwrap()).abs() < 1e-9);
    }
}

// The golden `energy_rounding` section replaces the older `truncation` one for
// this crate. `truncation` pinned `int(e*1000)` truncating toward zero, and the
// Rust half of it asserted `(e * 1000.0) as i64` inline — arithmetic in the test,
// not a call into the code under test, so it could not catch the scoring bug it
// looked like it covered. `energy_milli` no longer truncates a f64 accumulator:
// it recovers each coefficient's integer milli by rounding to nearest, which is
// what makes it agree with the coordinator. These cases run that real entry point
// and pin the rounded result. The `truncation` section stays in the fixture for
// the Python and JS runners, which assert their own language's cast primitive.
#[test]
fn energy_rounding_matches_golden() {
    for (index, case) in golden()["energy_rounding"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let energy = case["energy"].as_f64().unwrap();
        assert_eq!(
            energy_milli(&[1], &[energy], &[], &[]),
            case["energy_milli"].as_i64().unwrap(),
            "energy_rounding case {index}"
        );
    }
}

/// JSON has no literal for infinity or NaN, so the `sentinel` section spells them
/// as strings; anything else is an ordinary number.
#[expect(
    clippy::unwrap_used,
    reason = "integration-test helper; fixture shape is fixed"
)]
fn non_finite_coefficients_of(case: &Value, key: &str) -> Vec<f64> {
    case[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| match v.as_str() {
            Some("inf") => f64::INFINITY,
            Some("-inf") => f64::NEG_INFINITY,
            Some("nan") => f64::NAN,
            Some(_) | None => v.as_f64().unwrap(),
        })
        .collect()
}

// Pins the non-finite sentinel so no binding can pick a different value. It must
// stay distinct from the i64::MAX/MIN saturation results, which mean "finite but
// out of range" rather than "malformed problem".
#[test]
fn sentinel_matches_golden() {
    let g = golden();
    let cases = g["sentinel"].as_array().unwrap();
    assert!(!cases.is_empty(), "sentinel section must not be empty");
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let spins = spins_of(case);
        let h = non_finite_coefficients_of(case, "h");
        let j = non_finite_coefficients_of(case, "j");
        let edges = edges_of(case);
        let expected = case["energy_milli"].as_i64().unwrap();
        assert_eq!(
            expected, ENERGY_MILLI_NON_FINITE,
            "sentinel case {name} pins a value other than the exported constant"
        );
        assert_eq!(
            energy_milli(&spins, &h, &j, &edges),
            expected,
            "sentinel case {name}"
        );
    }
}
