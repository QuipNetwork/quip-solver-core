//! The scalar `ChaCha8` stream checked against the legacy golden vectors.

use ising_generators::legacy::{derive_nonce, NonceDeriver};
use ising_generators::rng::{key_words, ChaCha8Rng, SIGMA};
use quip_protocol::chacha8::{draw_into, draw_ising_milli};
use quip_protocol::lease::{Generator, LeaseSpec, TopologyView};
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

/// The scalar reference draw: `allowed[next_u32 % len]`, fields then couplings.
fn scalar_draw(
    nonce: [u8; 32],
    n_nodes: usize,
    n_edges: usize,
    allowed_h: &[i32],
    allowed_j: &[i32],
) -> Result<(Vec<i32>, Vec<i32>), String> {
    let mut rng = ChaCha8Rng::from_seed(nonce);
    let mut draw = |allowed: &[i32], count: usize| -> Result<Vec<i32>, String> {
        let len = u32::try_from(allowed.len()).map_err(|e| e.to_string())?;
        if count > 0 && len == 0 {
            return Err("empty allowed set".into());
        }
        (0..count)
            .map(|_| {
                let index = usize::try_from(rng.next_u32() % len).map_err(|e| e.to_string())?;
                allowed
                    .get(index)
                    .copied()
                    .ok_or_else(|| "index out of range".to_string())
            })
            .collect()
    };
    let h = draw(allowed_h, n_nodes)?;
    let j = draw(allowed_j, n_edges)?;
    Ok((h, j))
}

/// BLAKE3 hex of the little-endian bytes of `values`.
fn le_hash(values: &[i32]) -> String {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    blake3::hash(&bytes).to_hex().to_string()
}

fn i32_list(case: &Value, key: &str) -> Result<Vec<i32>, String> {
    case.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing array {key}"))?
        .iter()
        .map(|v| {
            v.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| format!("{key}: not an i32"))
        })
        .collect()
}

fn len_of(case: &Value, key: &str) -> Result<usize, String> {
    Ok(case
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing array {key}"))?
        .len())
}

#[test]
fn golden_ising_cases_agree_three_ways() -> Result<(), String> {
    let doc = golden()?;
    let cases = doc
        .get("ising")
        .and_then(Value::as_array)
        .ok_or("missing ising array")?;
    if cases.is_empty() {
        return Err("no ising cases".into());
    }
    for (i, case) in cases.iter().enumerate() {
        let nonce = hex32(str_field(case, "nonce_hex")?)?;
        let allowed_h = i32_list(case, "allowed_h_milli")?;
        let allowed_j = i32_list(case, "allowed_j_milli")?;
        let want_h = i32_list(case, "h_milli")?;
        let want_j = i32_list(case, "j_milli")?;
        let n_nodes = len_of(case, "nodes")?;
        let n_edges = len_of(case, "edges")?;
        let scalar = scalar_draw(nonce, n_nodes, n_edges, &allowed_h, &allowed_j)?;
        let fast = draw_ising_milli(nonce, n_nodes, n_edges, &allowed_h, &allowed_j)
            .map_err(|e| e.to_string())?;
        if scalar != (want_h.clone(), want_j.clone()) {
            return Err(format!("case {i}: scalar differs from golden"));
        }
        if fast != (want_h, want_j) {
            return Err(format!("case {i}: accelerated differs from golden"));
        }
    }
    Ok(())
}

#[test]
fn golden_lease_cases_agree_three_ways() -> Result<(), String> {
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
        let n_nodes = count_of(case, "n_nodes")?;
        let n_edges = count_of(case, "n_edges")?;
        let allowed_h = i32_list(case, "allowed_h_milli")?;
        let allowed_j = i32_list(case, "allowed_j_milli")?;
        let salt_start: u64 = str_field(case, "salt_start")?
            .parse()
            .map_err(|e| format!("{name}: salt_start: {e}"))?;
        let salt_count = case
            .get("salt_count")
            .and_then(Value::as_u64)
            .ok_or("no salt_count")?;
        let spec = LeaseSpec::new(
            Generator::Blake3Chacha8V1,
            hex32(str_field(case, "last_proof_block_hash")?)?,
            hex32(str_field(case, "miner_account")?)?,
            hex32(str_field(case, "base_salt")?)?,
            salt_start,
            salt_count,
        )
        .map_err(|e| format!("{name}: {e}"))?;
        // Topology edges k = (k mod n, (k + 1) mod n); the draw reads only the count.
        let topology = TopologyView {
            num_nodes: n_nodes,
            edges: (0..n_edges)
                .map(|k| (k % n_nodes, (k + 1) % n_nodes))
                .collect(),
            allowed_h_milli: allowed_h.clone(),
            allowed_j_milli: allowed_j.clone(),
        };
        let salts = case
            .get("salts")
            .and_then(Value::as_array)
            .ok_or("missing salts array")?;
        if u64::try_from(salts.len()).map_err(|e| e.to_string())? != salt_count {
            return Err(format!("{name}: salt count differs from salts length"));
        }
        for (index, entry) in salts.iter().enumerate() {
            let nonce = hex32(str_field(entry, "nonce")?)?;
            let index = u64::try_from(index).map_err(|e| e.to_string())?;
            if spec.nonce(index) != Some(nonce) {
                return Err(format!("{name} {index}: lease nonce differs"));
            }
            let scalar = scalar_draw(nonce, n_nodes, n_edges, &allowed_h, &allowed_j)?;
            let fast = topology.draw(nonce).map_err(|e| e.to_string())?;
            for (label, (h, j)) in [("scalar", &scalar), ("accelerated", &fast)] {
                if le_hash(h) != str_field(entry, "h_blake3")? {
                    return Err(format!("{name} {index}: {label} h hash differs"));
                }
                if le_hash(j) != str_field(entry, "j_blake3")? {
                    return Err(format!("{name} {index}: {label} j hash differs"));
                }
            }
        }
    }
    Ok(())
}

fn count_of(case: &Value, key: &str) -> Result<usize, String> {
    let n = case
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("missing count {key}"))?;
    usize::try_from(n).map_err(|e| e.to_string())
}

#[test]
fn dispatched_draw_matches_scalar_stream() -> Result<(), String> {
    let mut seeds = vec![[0x5A; 32], [0xA5; 32]];
    let doc = golden()?;
    for case in doc
        .get("chacha8")
        .and_then(Value::as_array)
        .ok_or("missing chacha8 array")?
    {
        seeds.push(hex32(str_field(case, "seed_hex")?)?);
    }
    let table: Vec<u32> = (0..65536).collect();
    for seed in seeds {
        for first_word in [0_u64, 1, 15, 16, 17, 127, 128, 129, 1000] {
            for len in [1_usize, 15, 16, 17, 128, 129, 4096] {
                let mut out = vec![0_u32; len];
                draw_into(seed, first_word, &table, &mut out).map_err(|e| e.to_string())?;
                let mut reference = ChaCha8Rng::from_seed(seed);
                for _ in 0..first_word {
                    let _ = reference.next_u32();
                }
                for (i, got) in out.iter().enumerate() {
                    let want = reference.next_u32() % 65536;
                    if *got != want {
                        return Err(format!(
                            "seed {:02x}{:02x}, first_word {first_word}, len {len}, word {i}: got {got}, want {want}",
                            seed[0], seed[1]
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
#[expect(
    clippy::print_stderr,
    reason = "the test must tell the reader when the AVX2 path did not run"
)]
fn dispatched_path_is_reported() {
    #[cfg(target_arch = "x86_64")]
    let avx2 = is_x86_feature_detected!("avx2");
    #[cfg(not(target_arch = "x86_64"))]
    let avx2 = false;
    if avx2 {
        eprintln!("AVX2 path exercised by the dispatched draw");
    } else {
        eprintln!("SKIPPED: AVX2 path not exercised, the dispatched path is the four-lane path");
    }
}

/// Seed `000102...1f`, the first golden `chacha8` case.
const fn vector_seed() -> [u8; 32] {
    [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 30, 31,
    ]
}

fn set_draw(set: &[i32]) -> Result<[i32; 4], String> {
    let mut out = [0_i32; 4];
    draw_into(vector_seed(), 0, set, &mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

macro_rules! expect_eq {
    ($label:expr, $got:expr, $want:expr $(,)?) => {{
        let (got, want) = (&$got, &$want);
        if got == want {
            Ok(())
        } else {
            Err(format!("{}: got {:?}, want {:?}", $label, got, want))
        }
    }};
}

#[test]
fn validator_set_vectors() -> Result<(), String> {
    // Semantics: quip-validator commit 1f8a1c0, `AllowedValueSpec::sample`, `Set`:
    // `values[next_u32 % len]`.
    expect_eq!(
        "Set [-1000, 0, 1000]",
        set_draw(&[-1000, 0, 1000])?,
        [0, -1000, 1000, -1000],
    )?;
    expect_eq!(
        "Set [-1000, 1000]",
        set_draw(&[-1000, 1000])?,
        [-1000, -1000, 1000, 1000],
    )
}

#[test]
fn validator_empty_set_is_an_error() -> Result<(), String> {
    match draw_ising_milli([0; 32], 1, 0, &[], &[]) {
        Err(_) => Ok(()),
        Ok(_) => Err("an empty allowed_h with one node was accepted".into()),
    }
}

#[test]
fn single_value_set_still_consumes_words() -> Result<(), String> {
    let nonce = vector_seed();
    let allowed_j = [-1000, 0, 1000];
    let (h, j) = draw_ising_milli(nonce, 3, 4, &[500], &allowed_j).map_err(|e| e.to_string())?;
    expect_eq!("h", h, vec![500, 500, 500])?;
    let mut rng = ChaCha8Rng::from_seed(nonce);
    for _ in 0..3 {
        let _ = rng.next_u32();
    }
    let want: Vec<i32> = (0..4)
        .map(|_| {
            let index = usize::try_from(rng.next_u32() % 3).map_err(|e| e.to_string())?;
            allowed_j
                .get(index)
                .copied()
                .ok_or_else(|| format!("index {index} outside the allowed set"))
        })
        .collect::<Result<_, String>>()?;
    expect_eq!("j", j, want)
}

#[test]
fn validator_range_vectors() -> Result<(), String> {
    // Semantics: quip-validator commit 1f8a1c0, `AllowedValueSpec::sample`.
    // IntegerRange: `(min + word % (max - min + 1)) * 1000`, ArithmeticOverflow
    // outside i32. ContinuousRange: `min + word % (max - min + 1)` in milli.
    let integer: Vec<i32> = (-6..=6).map(|v| v * 1000).collect();
    expect_eq!(
        "IntegerRange {-6, 6}",
        set_draw(&integer)?,
        [1000, 1000, -2000, -3000],
    )?;
    expect_eq!(
        "endpoints [-6000, 6000]",
        set_draw(&[-6000, 6000])?,
        [-6000, -6000, 6000, 6000],
    )?;
    let continuous: Vec<i32> = (-1500..=1500).collect();
    expect_eq!(
        "ContinuousRange {-1500, 1500}",
        set_draw(&continuous)?,
        [-100, -563, 1466, 118],
    )?;
    expect_eq!(
        "endpoints [-1500, 1500]",
        set_draw(&[-1500, 1500])?,
        [-1500, -1500, 1500, 1500],
    )?;
    if set_draw(&integer)? == set_draw(&[-6000, 6000])?
        || set_draw(&continuous)? == set_draw(&[-1500, 1500])?
    {
        return Err("a range must not equal its endpoints-only list".into());
    }

    // The next two rows have no `Set` equivalent. They are literal pins of the
    // validator output at commit 1f8a1c0, over the first four golden words.
    let words = [2_410_812_736_u32, 1_789_596_270, 2_070_341_849, 856_868_145];
    let mut rng = ChaCha8Rng::from_seed(vector_seed());
    for (i, want) in words.iter().enumerate() {
        expect_eq!(&format!("golden word {i}"), rng.next_u32(), *want)?;
    }
    // ContinuousRange { i32::MIN, i32::MAX }: span 2^32, so offset == word.
    let full_span: Vec<i64> = words
        .iter()
        .map(|w| i64::from(i32::MIN) + i64::from(*w))
        .collect();
    expect_eq!(
        "ContinuousRange i32 span",
        full_span,
        vec![263_329_088, -357_887_378, -77_141_799, -1_290_615_503],
    )?;
    // IntegerRange { 2147483, 2147484 }: span 2, top value overflows i32 at * 1000.
    let overflow: Vec<Option<i32>> = words
        .iter()
        .map(|w| {
            let value = (2_147_483_i64 + i64::from(*w % 2)) * 1000;
            i32::try_from(value).ok()
        })
        .collect();
    expect_eq!(
        "IntegerRange overflow",
        overflow,
        vec![Some(2_147_483_000), Some(2_147_483_000), None, None],
    )
}
