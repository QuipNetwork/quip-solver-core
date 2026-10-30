//! Family primitives checked against `vectors/families/core.json`.

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

#[expect(dead_code, reason = "shared helper used by later tasks")]
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
