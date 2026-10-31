//! FAM-42 code to wire reason mapping against the `core.json` `codes` rows.
#![cfg(feature = "session")]

use ising_generators::Code;
use quip_protocol::family::{reject_reason, CodePhase};
use serde_json::Value;

fn codes() -> Result<Vec<Value>, String> {
    let doc: Value =
        serde_json::from_str(quip_solver_conformance::FAMILY_CORE).map_err(|e| e.to_string())?;
    doc.get("codes")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "codes is missing".to_owned())
}

fn row_for(rows: &[Value], code: Code) -> Result<&Value, String> {
    rows.iter()
        .find(|r| r.get("code").and_then(Value::as_u64) == Some(u64::from(code.value())))
        .ok_or_else(|| format!("no codes row for {code}"))
}

fn column(row: &Value, key: &str) -> Result<Option<String>, String> {
    match row.get(key) {
        Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        _ => Err(format!("bad column {key}")),
    }
}

#[test]
fn reject_reasons_match_the_codes_vectors() -> Result<(), String> {
    let rows = codes()?;
    if Code::ALL.len() != 16 || rows.len() != 16 {
        return Err("expected 16 codes and 16 rows".to_owned());
    }
    for code in Code::ALL {
        let row = row_for(&rows, code)?;
        for (phase, key) in [
            (CodePhase::Admission, "wire_admission"),
            (CodePhase::ExplicitJob, "wire_explicit_job"),
            (CodePhase::Generation, "wire_generation"),
        ] {
            let got = reject_reason(code, phase).map(|r| r.as_str_name().to_owned());
            let want = column(row, key)?;
            if got != want {
                return Err(format!("{code} {phase:?}: got {got:?}, want {want:?}"));
            }
        }
    }
    Ok(())
}

#[test]
fn generation_never_rejects() {
    for code in Code::ALL {
        assert_eq!(reject_reason(code, CodePhase::Generation), None, "{code}");
    }
}
