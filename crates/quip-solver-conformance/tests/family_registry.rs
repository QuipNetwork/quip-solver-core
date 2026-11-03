//! The family registry and the FAM-38 contracts.

use ising_generators::descriptor::{check_family_id, Descriptor, IdContext};
use quip_solver_conformance::families::registry::{parse_registry, RegistryEntry, Status};
use quip_solver_conformance::{FAMILY_LAYERED, FAMILY_REGISTRY};
use serde_json::Value;

fn registry() -> Result<Vec<RegistryEntry>, String> {
    parse_registry(FAMILY_REGISTRY)
}

/// The vectors file of an entry, read from the repository root at run time.
fn vectors_of(entry: &RegistryEntry) -> Result<Value, String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join(&entry.vectors))
        .map_err(|e| format!("{}: {e}", entry.vectors))?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn layered_cases(doc: &Value) -> Result<&Vec<Value>, String> {
    doc.get("layered")
        .and_then(Value::as_array)
        .ok_or_else(|| "no layered section".to_owned())
}

fn case_key(case: &Value) -> Result<(String, u32), String> {
    let id = case
        .get("family_id")
        .and_then(Value::as_str)
        .ok_or("case without family_id")?;
    let revision = case
        .get("revision")
        .and_then(Value::as_u64)
        .and_then(|r| u32::try_from(r).ok())
        .ok_or("case without revision")?;
    Ok((id.to_owned(), revision))
}

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| format!("bad hex at {i}"))
        })
        .collect()
}

#[test]
fn registry_entries_are_unique_and_valid() -> Result<(), String> {
    let registry = registry()?;
    let keys: Vec<(&str, u32)> = registry
        .iter()
        .map(|e| (e.family_id.as_str(), e.revision))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    sorted.dedup();
    if keys != sorted {
        return Err(format!("entries not unique and sorted: {keys:?}"));
    }
    for entry in &registry {
        let id = &entry.family_id;
        check_family_id(id.as_bytes(), IdContext::Lease)
            .map_err(|code| format!("{id}: family id rejected with {code:?}"))?;
        if entry.revision < 1 {
            return Err(format!("{id}: revision is zero"));
        }
        let contract = format!(
            "docs/families/{}-{}.md",
            id.replace('/', "-"),
            entry.revision
        );
        if entry.contract != contract {
            return Err(format!("{id}: contract {} != {contract}", entry.contract));
        }
    }
    let doc: Value = serde_json::from_str(FAMILY_REGISTRY).map_err(|e| e.to_string())?;
    let names: Vec<&str> = doc
        .get("registry")
        .and_then(Value::as_array)
        .ok_or("no registry")?
        .iter()
        .map(|e| e.get("name").and_then(Value::as_str).unwrap_or(""))
        .collect();
    let want: Vec<String> = registry
        .iter()
        .map(|e| format!("{}@{}", e.family_id, e.revision))
        .collect();
    if names != want {
        return Err(format!("names {names:?} != {want:?}"));
    }
    Ok(())
}

#[test]
fn every_registered_revision_has_vectors() -> Result<(), String> {
    let registry = registry()?;
    for entry in &registry {
        let doc = vectors_of(entry)?;
        let mut found = false;
        for case in layered_cases(&doc)? {
            found |= case_key(case)? == (entry.family_id.clone(), entry.revision);
        }
        if !found {
            return Err(format!("{}@{}: no cases", entry.family_id, entry.revision));
        }
    }
    let layered: Value = serde_json::from_str(FAMILY_LAYERED).map_err(|e| e.to_string())?;
    for case in layered_cases(&layered)? {
        let (id, revision) = case_key(case)?;
        if !registry
            .iter()
            .any(|e| e.family_id == id && e.revision == revision)
        {
            return Err(format!(
                "{id}@{revision} is in layered.json, not registered"
            ));
        }
    }
    Ok(())
}

/// SC-7 #4. The digest function takes no status, so each status gives the
/// stored digest and a promotion keeps it.
#[test]
fn status_does_not_change_the_descriptor_digest() -> Result<(), String> {
    for entry in &registry()? {
        let doc = vectors_of(entry)?;
        for case in layered_cases(&doc)? {
            if case_key(case)? != (entry.family_id.clone(), entry.revision) {
                continue;
            }
            let params = hex_decode(
                case.get("params_hex")
                    .and_then(Value::as_str)
                    .ok_or("no params_hex")?,
            )?;
            let stored = case
                .get("descriptor_digest")
                .and_then(Value::as_str)
                .ok_or("no descriptor_digest")?;
            for status in [Status::Research, Status::Archived, Status::ChainAuthorized] {
                let mut promoted = entry.clone();
                promoted.status = status;
                let descriptor = Descriptor::new(
                    promoted.family_id.as_bytes(),
                    promoted.revision,
                    &params,
                    IdContext::Lease,
                )
                .map_err(|code| format!("{code:?}"))?;
                let digest = descriptor.digest();
                if digest.as_slice() != hex_decode(stored)? {
                    return Err(format!("{status:?}: digest differs from {stored}"));
                }
            }
        }
    }
    Ok(())
}
