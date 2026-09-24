//! Lease salt, nonce, and full draw hashes from the validator crate.
#![expect(
    clippy::indexing_slicing,
    reason = "golden JSON keys are fixed by the fixture"
)]

use quip_protocol::lease::{Generator, LeaseSpec, TopologyView};
use serde_json::Value;

#[expect(clippy::unwrap_used, reason = "fixture shape is fixed")]
fn golden() -> Value {
    serde_json::from_str(quip_solver_conformance::GOLDEN_LEASE).unwrap()
}

#[expect(clippy::unwrap_used, reason = "fixture hex encodes exactly 32 bytes")]
fn bytes(value: &Value) -> [u8; 32] {
    let hex = value.as_str().unwrap();
    assert_eq!(hex.len(), 64);
    let decoded: Vec<_> = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    decoded.try_into().unwrap()
}

fn hash(values: &[i32]) -> String {
    let bytes: Vec<_> = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    blake3::hash(&bytes).to_hex().to_string()
}

#[test]
fn lease_draws_match_chain() {
    let fixture = golden();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 3);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let spec = LeaseSpec::new(
            Generator::Blake3Chacha8V1,
            bytes(&case["last_proof_block_hash"]),
            bytes(&case["miner_account"]),
            bytes(&case["base_salt"]),
            case["salt_start"].as_str().unwrap().parse().unwrap(),
            case["salt_count"].as_u64().unwrap(),
        )
        .unwrap();
        let num_nodes = usize::try_from(case["n_nodes"].as_u64().unwrap()).unwrap();
        let num_edges = usize::try_from(case["n_edges"].as_u64().unwrap()).unwrap();
        let topology = TopologyView {
            num_nodes,
            edges: (0..num_edges)
                .map(|k| (k % num_nodes, (k + 1) % num_nodes))
                .collect(),
            allowed_h_milli: serde_json::from_value(case["allowed_h_milli"].clone()).unwrap(),
            allowed_j_milli: serde_json::from_value(case["allowed_j_milli"].clone()).unwrap(),
        };
        let salts = case["salts"].as_array().unwrap();
        assert_eq!(
            u64::try_from(salts.len()).unwrap(),
            spec.salt_count,
            "{name}"
        );
        for (index, vector) in salts.iter().enumerate() {
            let index = u64::try_from(index).unwrap();
            let salt = bytes(&vector["salt"]);
            let nonce = bytes(&vector["nonce"]);
            assert_eq!(spec.salt(index), Some(salt), "{name} salt {index}");
            assert_eq!(
                spec.index_of(&salt),
                Some(index),
                "{name} salt index {index}"
            );
            assert_eq!(spec.nonce(index), Some(nonce), "{name} nonce {index}");
            let (h, j) = topology.draw(nonce).unwrap();
            assert_eq!(h.len(), num_nodes, "{name} field count {index}");
            assert_eq!(j.len(), num_edges, "{name} coupling count {index}");
            assert_eq!(
                hash(&h),
                vector["h_blake3"].as_str().unwrap(),
                "{name} h {index}"
            );
            assert_eq!(
                hash(&j),
                vector["j_blake3"].as_str().unwrap(),
                "{name} j {index}"
            );
        }
    }
}
