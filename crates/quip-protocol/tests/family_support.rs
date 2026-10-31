//! FAM-60, FAM-61 and FAM-62 checks against the `core.json` support vectors.
#![cfg(feature = "session")]

use ising_generators::Code;
use quip_proto::v1::FamilySupport;
use quip_protocol::family::{entry_claims, solver_id_class, support_code, SolverIdClass};
use serde_json::Value;

fn section(name: &str) -> Result<Vec<Value>, String> {
    let doc: Value =
        serde_json::from_str(quip_solver_conformance::FAMILY_CORE).map_err(|e| e.to_string())?;
    doc.get(name)
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| format!("{name} is missing"))
}

fn text<'a>(case: &'a Value, key: &str) -> Result<&'a str, String> {
    case.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} is missing"))
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err(format!("odd hex length in {hex}"));
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| format!("bad hex in {hex}"))
        })
        .collect()
}

fn support_list(entries: &Value) -> Result<Vec<FamilySupport>, String> {
    let entries = entries.as_array().ok_or("entries is not an array")?;
    entries
        .iter()
        .map(|entry| {
            let revisions = entry
                .get("revisions")
                .and_then(Value::as_array)
                .ok_or("entry has no revisions")?
                .iter()
                .map(|r| {
                    r.as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .ok_or_else(|| "bad revision".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            let params_digests = entry
                .get("params_digests")
                .and_then(Value::as_array)
                .ok_or("entry has no params_digests")?
                .iter()
                .map(|d| hex_decode(d.as_str().ok_or("digest is not a string")?))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(FamilySupport {
                family_id: text(entry, "family_id")?.to_owned(),
                revisions,
                params_digests,
            })
        })
        .collect()
}

fn entry(family_id: &str, revisions: &[u32], digests: &[[u8; 32]]) -> FamilySupport {
    FamilySupport {
        family_id: family_id.to_owned(),
        revisions: revisions.to_vec(),
        params_digests: digests.iter().map(|d| d.to_vec()).collect(),
    }
}

#[test]
fn solver_ids_match_the_vectors() -> Result<(), String> {
    for case in section("solver_ids")? {
        let name = text(&case, "name")?;
        let id = hex_decode(text(&case, "solver_id_hex")?)?;
        let expected = match text(&case, "expected")? {
            "valid" => SolverIdClass::Valid,
            "invalid" => SolverIdClass::Invalid,
            "empty" => SolverIdClass::Empty,
            other => return Err(format!("{name}: unknown class {other}")),
        };
        if solver_id_class(&id) != expected {
            return Err(format!("{name}: class differs from {expected:?}"));
        }
    }
    Ok(())
}

#[test]
fn entry_claims_match_the_vectors() -> Result<(), String> {
    for case in section("family_support")? {
        let name = text(&case, "name")?;
        let list = support_list(case.get("entries").ok_or("entries is missing")?)?;
        let expected: Vec<bool> = case
            .get("entry_claims")
            .and_then(Value::as_array)
            .ok_or("entry_claims is missing")?
            .iter()
            .map(|b| b.as_bool().ok_or_else(|| "bad claim".to_owned()))
            .collect::<Result<_, _>>()?;
        if entry_claims(&list) != expected {
            return Err(format!("{name}: claims differ"));
        }
    }
    Ok(())
}

#[test]
fn support_codes_match_the_lease_codes_vectors() -> Result<(), String> {
    for case in section("lease_codes")? {
        let name = text(&case, "name")?;
        let list = support_list(
            case.get("generated_families")
                .ok_or("generated_families is missing")?,
        )?;
        let revision = case
            .get("revision")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("bad revision")?;
        let digest = hex_decode(text(&case, "params_digest")?)?;
        let expected = match case.get("expected_code") {
            Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().ok_or("bad expected_code")?),
            None => return Err(format!("{name}: expected_code is missing")),
        };
        let actual = support_code(&list, text(&case, "family_id")?, revision, &digest)
            .map(|c| u64::from(c.value()));
        if actual != expected {
            return Err(format!("{name}: got {actual:?}, expected {expected:?}"));
        }
    }
    Ok(())
}

#[test]
fn an_empty_list_claims_no_family() {
    assert!(entry_claims(&[]).is_empty());
    assert_eq!(
        support_code(&[], "quip/x", 1, &[0; 32]),
        Some(Code::FamilyUnsupported)
    );
}

#[test]
fn an_invalid_entry_claims_nothing() {
    let example = [entry("example/foo", &[1], &[])];
    assert_eq!(
        support_code(&example, "example/foo", 1, &[0; 32]),
        Some(Code::FamilyUnsupported)
    );
    let duplicate = [entry("quip/x", &[1], &[]), entry("quip/x", &[2], &[])];
    assert_eq!(
        support_code(&duplicate, "quip/x", 1, &[0; 32]),
        Some(Code::FamilyUnsupported)
    );
}

#[test]
fn a_restriction_admits_only_its_digests() {
    let open = [entry("quip/x", &[1], &[])];
    assert_eq!(support_code(&open, "quip/x", 1, &[0xbb; 32]), None);
    let restricted = [entry("quip/x", &[1], &[[0xaa; 32]])];
    assert_eq!(support_code(&restricted, "quip/x", 1, &[0xaa; 32]), None);
    assert_eq!(
        support_code(&restricted, "quip/x", 1, &[0xbb; 32]),
        Some(Code::ParamsRestricted)
    );
}
