//! Family primitives checked against `vectors/families/core.json`.

use core::num::NonZeroU32;

use ising_generators::rng::{uniform, ChaCha8Rng};
use ising_generators::Code;
use serde_json::Value;

fn core() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::FAMILY_CORE).map_err(|e| e.to_string())
}

fn section<'a>(doc: &'a Value, name: &str) -> Result<&'a [Value], String> {
    doc.get(name)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing array section {name}"))
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
fn codes_match_the_fixture_table() -> Result<(), String> {
    let doc = core()?;
    let rows = section(&doc, "codes")?;
    if rows.len() != 16 || Code::ALL.len() != 16 {
        return Err(format!(
            "expected 16 codes, fixture {} ALL {}",
            rows.len(),
            Code::ALL.len()
        ));
    }
    if !Code::ALL
        .windows(2)
        .all(|w| matches!(w, [a, b] if a.value() < b.value()))
    {
        return Err("Code::ALL is not ascending".into());
    }
    for row in rows {
        let value = row
            .get("code")
            .and_then(Value::as_u64)
            .ok_or("row without code")?;
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .ok_or("row without name")?;
        let found = Code::ALL
            .iter()
            .find(|c| u64::from(c.value()) == value)
            .ok_or_else(|| format!("no variant for code {value}"))?;
        if found.to_string() != name {
            return Err(format!("code {value}: display {found} but fixture {name}"));
        }
    }
    Ok(())
}

fn nz(n: u64) -> Result<NonZeroU32, String> {
    u32::try_from(n)
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| format!("n {n} is not a non-zero u32"))
}

#[test]
fn uniform_vectors() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "uniform")? {
        let seed = hex32(case.get("seed").and_then(Value::as_str).ok_or("no seed")?)?;
        let calls = case
            .get("calls")
            .and_then(Value::as_array)
            .ok_or("no calls")?;
        let mut stream = ChaCha8Rng::from_seed(seed);
        let words: Vec<u32> = (0..calls.len() * 64 + 64)
            .map(|_| stream.next_u32())
            .collect();
        for k in 0..calls.len() {
            let mut rng = ChaCha8Rng::from_seed(seed);
            for (i, call) in calls.iter().enumerate().take(k + 1) {
                let n = nz(call.get("n").and_then(Value::as_u64).ok_or("no n")?)?;
                let want = call
                    .get("result")
                    .and_then(Value::as_u64)
                    .ok_or("no result")?;
                let got = uniform(&mut rng, n);
                if u64::from(got) != want {
                    return Err(format!("call {i}: got {got}, want {want}"));
                }
            }
            let used = call_words_used(calls, k)?;
            let next = rng.next_u32();
            if words.get(used) != Some(&next) {
                return Err(format!(
                    "call {k}: words_used {used} does not match the stream"
                ));
            }
        }
    }
    Ok(())
}

fn call_words_used(calls: &[Value], k: usize) -> Result<usize, String> {
    let v = calls
        .get(k)
        .and_then(|c| c.get("words_used"))
        .and_then(Value::as_u64)
        .ok_or("no words_used")?;
    usize::try_from(v).map_err(|e| e.to_string())
}

#[test]
fn uniform_of_one_reads_one_word() -> Result<(), String> {
    let seed = [7u8; 32];
    let mut a = ChaCha8Rng::from_seed(seed);
    let mut b = ChaCha8Rng::from_seed(seed);
    let one = nz(1)?;
    if uniform(&mut a, one) != 0 {
        return Err("uniform(1) is not 0".into());
    }
    let _ = b.next_u32();
    if a.next_u32() != b.next_u32() {
        return Err("uniform(1) did not read exactly one word".into());
    }
    Ok(())
}
