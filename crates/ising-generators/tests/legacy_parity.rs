//! The scalar `ChaCha8` stream checked against the legacy golden vectors.

use ising_generators::rng::{key_words, ChaCha8Rng, SIGMA};
use serde_json::Value;

fn golden() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::GOLDEN_VECTORS).map_err(|e| e.to_string())
}

#[expect(dead_code, reason = "parsed here for later tasks in this file")]
fn golden_lease() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::GOLDEN_LEASE).map_err(|e| e.to_string())
}

fn hex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err(format!("odd hex length {}", s.len()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i + 2)
                .ok_or_else(|| "short hex".to_string())
                .and_then(|p| u8::from_str_radix(p, 16).map_err(|e| e.to_string()))
        })
        .collect()
}

fn hex32(s: &str) -> Result<[u8; 32], String> {
    hex(s)?
        .try_into()
        .map_err(|v: Vec<u8>| format!("expected 32 bytes, got {}", v.len()))
}

#[test]
fn scalar_chacha8_matches_golden_words() -> Result<(), String> {
    let doc = golden()?;
    let cases = doc
        .get("chacha8")
        .and_then(Value::as_array)
        .ok_or("missing chacha8 array")?;
    if cases.is_empty() {
        return Err("no chacha8 cases".into());
    }
    for case in cases {
        let seed = hex32(
            case.get("seed_hex")
                .and_then(Value::as_str)
                .ok_or("no seed_hex")?,
        )?;
        let n = case
            .get("n_words")
            .and_then(Value::as_u64)
            .ok_or("no n_words")?;
        let words = case
            .get("words")
            .and_then(Value::as_array)
            .ok_or("no words")?;
        if u64::try_from(words.len()).map_err(|e| e.to_string())? != n {
            return Err("n_words differs from words length".into());
        }
        let mut rng = ChaCha8Rng::from_seed(seed);
        for (i, w) in words.iter().enumerate() {
            let want = w.as_u64().ok_or("word is not an integer")?;
            let got = u64::from(rng.next_u32());
            if got != want {
                return Err(format!("word {i}: got {got}, want {want}"));
            }
        }
    }
    Ok(())
}

#[test]
fn key_words_are_the_seed_as_le_words() -> Result<(), String> {
    let seed = hex32("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")?;
    let words = key_words(&seed);
    if words.first() != Some(&0x0302_0100) || words.last() != Some(&0x1f1e_1d1c) {
        return Err(format!("unexpected key words {words:x?}"));
    }
    Ok(())
}

#[test]
fn sigma_is_expand_32_byte_k() -> Result<(), String> {
    let bytes: Vec<u8> = SIGMA.iter().flat_map(|w| w.to_le_bytes()).collect();
    if bytes != b"expand 32-byte k" {
        return Err(format!("unexpected sigma bytes {bytes:?}"));
    }
    Ok(())
}
