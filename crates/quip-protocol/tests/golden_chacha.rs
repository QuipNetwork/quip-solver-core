//! Golden `ChaCha8` keystream, Ising draw order, and nonce derivation vs
//! `conformance/golden_vectors.json`.
#![expect(
    clippy::indexing_slicing,
    reason = "golden JSON keys and 64-char hex slices are fixed by the fixture"
)]
#![expect(
    clippy::cast_possible_truncation,
    reason = "golden milli values fit in i32 by fixture contract"
)]

use quip_protocol::chacha8::{draw_ising_milli, ChaCha8Rng};
use quip_protocol::derive::derive_nonce;
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
    reason = "integration-test helper; golden hex is always valid 32-byte seed"
)]
fn hex32(s: &str) -> [u8; 32] {
    let bytes: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect();
    bytes.try_into().unwrap()
}

// The `derive_nonce` section had no reader: the only check was a hardcoded copy
// of case[0] inside src/derive.rs, so editing the fixture could not fail a test
// and cases beyond the first were never executed. This loads the section and
// asserts every case, which is what makes the fixture the source of truth.
#[test]
fn derive_nonce_matches_golden() {
    let g = golden();
    let cases = g["derive_nonce"].as_array().unwrap();
    assert!(!cases.is_empty(), "derive_nonce section must not be empty");
    for (index, case) in cases.iter().enumerate() {
        let last_proof = hex32(case["last_proof_hex"].as_str().unwrap());
        let miner = hex32(case["miner_hex"].as_str().unwrap());
        let salt = hex32(case["salt_hex"].as_str().unwrap());
        let expected = hex32(case["nonce_hex"].as_str().unwrap());
        assert_eq!(
            derive_nonce(last_proof, miner, salt),
            expected,
            "derive_nonce case {index}"
        );
    }
}

#[test]
fn chacha8_keystream_matches_golden() {
    let g = golden();
    for case in g["chacha8"].as_array().unwrap() {
        let seed = hex32(case["seed_hex"].as_str().unwrap());
        let mut rng = ChaCha8Rng::from_seed(seed);
        let expected: Vec<u64> = case["words"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_u64().unwrap())
            .collect();
        for &w in &expected {
            assert_eq!(u64::from(rng.next_u32()), w);
        }
    }
}

#[test]
fn ising_draw_order_matches_golden() {
    let g = golden();
    for case in g["ising"].as_array().unwrap() {
        let nonce = hex32(case["nonce_hex"].as_str().unwrap());
        let n_nodes = case["nodes"].as_array().unwrap().len();
        let n_edges = case["edges"].as_array().unwrap().len();
        let allowed_h: Vec<i32> = case["allowed_h_milli"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap() as i32)
            .collect();
        let allowed_j: Vec<i32> = case["allowed_j_milli"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap() as i32)
            .collect();
        let (h, j) = draw_ising_milli(nonce, n_nodes, n_edges, &allowed_h, &allowed_j).unwrap();
        let exp_h: Vec<i32> = case["h_milli"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap() as i32)
            .collect();
        let exp_j: Vec<i32> = case["j_milli"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap() as i32)
            .collect();
        assert_eq!(h, exp_h);
        assert_eq!(j, exp_j);
    }
}
