//! SC-7 #1 and #28: the family declarations of every example miner agree with
//! the family registry.

mod common;

use common::example_bin;
use quip_solver_conformance::families::registry::{
    archived_among, parse_registry, unregistered_quip_ids, RegistryEntry,
};
use quip_solver_conformance::FAMILY_REGISTRY;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

/// The family declarations one example prints with `--capabilities`.
struct Declared {
    example: String,
    /// Every advertised `familyId`, generated and accepted.
    advertised: Vec<String>,
    /// Every generated `(familyId, revision)` pair.
    generated: Vec<(String, u32)>,
}

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples")
}

fn example_names() -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(examples_dir()).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|ext| ext == "rs") {
            if let Some(stem) = path.file_stem() {
                names.push(stem.to_string_lossy().into_owned());
            }
        }
    }
    names.sort();
    Ok(names)
}

fn claims<'a>(caps: &'a Value, key: &str) -> Result<&'a [Value], String> {
    match caps.get(key) {
        None => Ok(&[]),
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| format!("{key}: not an array")),
    }
}

fn family_id(claim: &Value) -> Result<&str, String> {
    claim
        .get("familyId")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("claim without familyId: {claim}"))
}

/// Arguments an example needs before it prints capabilities: the width-two
/// lease fixture requires `--gate` even though it never opens it here.
fn extra_args(example: &str) -> &'static [&'static str] {
    match example {
        "mock_sampler_lease_wide" => &["--gate", "/nonexistent"],
        _ => &[],
    }
}

fn declared(example: &str) -> Result<Declared, String> {
    let out = Command::new(example_bin(example))
        .args(extra_args(example))
        .arg("--capabilities")
        .output()
        .map_err(|e| format!("{example}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{example}: --capabilities failed"));
    }
    let caps: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("{example}: {e}"))?;
    let mut advertised = Vec::new();
    let mut generated = Vec::new();
    for claim in claims(&caps, "generatedFamilies")? {
        let id = family_id(claim)?;
        advertised.push(id.to_owned());
        let revisions = claim.get("revisions").and_then(Value::as_array);
        for revision in revisions.map(Vec::as_slice).unwrap_or_default() {
            let revision = revision
                .as_u64()
                .and_then(|r| u32::try_from(r).ok())
                .ok_or_else(|| format!("{example}: bad revision {revision}"))?;
            generated.push((id.to_owned(), revision));
        }
    }
    for claim in claims(&caps, "acceptedFamilies")? {
        advertised.push(family_id(claim)?.to_owned());
    }
    Ok(Declared {
        example: example.to_owned(),
        advertised,
        generated,
    })
}

fn all_declared() -> Result<(Vec<RegistryEntry>, Vec<Declared>), String> {
    let registry = parse_registry(FAMILY_REGISTRY)?;
    let names = example_names()?;
    if names.is_empty() {
        return Err("no examples found".to_owned());
    }
    let declared = names
        .iter()
        .map(|name| declared(name))
        .collect::<Result<Vec<_>, _>>()?;
    if !declared.iter().any(|d| !d.advertised.is_empty()) {
        return Err("no example advertises a family".to_owned());
    }
    if !declared.iter().any(|d| !d.generated.is_empty()) {
        return Err("no example generates a family".to_owned());
    }
    Ok((registry, declared))
}

#[test]
fn every_advertised_quip_family_is_registered() -> Result<(), String> {
    let (registry, all) = all_declared()?;
    let mut problems = Vec::new();
    for d in &all {
        let missing = unregistered_quip_ids(&registry, d.advertised.iter().map(String::as_str));
        if !missing.is_empty() {
            problems.push(format!("{}: unregistered {missing:?}", d.example));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}

#[test]
fn no_example_generates_an_archived_revision() -> Result<(), String> {
    let (registry, all) = all_declared()?;
    let mut problems = Vec::new();
    for d in &all {
        let archived = archived_among(&registry, &d.generated);
        if !archived.is_empty() {
            problems.push(format!("{}: archived {archived:?}", d.example));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("; "))
    }
}
