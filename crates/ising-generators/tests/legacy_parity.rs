//! The scalar `ChaCha8` stream checked against the legacy golden vectors.

use ising_generators::legacy::{derive_nonce, NonceDeriver};
use ising_generators::rng::{key_words, ChaCha8Rng, SIGMA};
use serde_json::Value;

fn golden() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::GOLDEN_VECTORS).map_err(|e| e.to_string())
}

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

fn str_field<'a>(case: &'a Value, key: &str) -> Result<&'a str, String> {
    case.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string field {key}"))
}

#[test]
fn derive_nonce_matches_golden() -> Result<(), String> {
    let doc = golden()?;
    let cases = doc
        .get("derive_nonce")
        .and_then(Value::as_array)
        .ok_or("missing derive_nonce array")?;
    if cases.is_empty() {
        return Err("no derive_nonce cases".into());
    }
    for (i, case) in cases.iter().enumerate() {
        let last = hex32(str_field(case, "last_proof_hex")?)?;
        let miner = hex32(str_field(case, "miner_hex")?)?;
        let salt = hex32(str_field(case, "salt_hex")?)?;
        let want = hex32(str_field(case, "nonce_hex")?)?;
        if derive_nonce(last, miner, salt) != want {
            return Err(format!("case {i}: derive_nonce differs"));
        }
        if NonceDeriver::new(last, miner).derive(salt) != want {
            return Err(format!("case {i}: NonceDeriver differs"));
        }
    }
    Ok(())
}

#[test]
fn lease_nonces_match_golden_lease() -> Result<(), String> {
    let doc = golden_lease()?;
    let cases = doc
        .get("cases")
        .and_then(Value::as_array)
        .ok_or("missing cases array")?;
    if cases.is_empty() {
        return Err("no lease cases".into());
    }
    for case in cases {
        let name = str_field(case, "name")?;
        let last = hex32(str_field(case, "last_proof_block_hash")?)?;
        let miner = hex32(str_field(case, "miner_account")?)?;
        let salts = case
            .get("salts")
            .and_then(Value::as_array)
            .ok_or("missing salts array")?;
        if salts.is_empty() {
            return Err(format!("{name}: no salts"));
        }
        let deriver = NonceDeriver::new(last, miner);
        for entry in salts {
            let salt = hex32(str_field(entry, "salt")?)?;
            let want = hex32(str_field(entry, "nonce")?)?;
            if derive_nonce(last, miner, salt) != want {
                return Err(format!("{name}: derive_nonce differs"));
            }
            if deriver.derive(salt) != want {
                return Err(format!("{name}: NonceDeriver differs"));
            }
        }
    }
    Ok(())
}
