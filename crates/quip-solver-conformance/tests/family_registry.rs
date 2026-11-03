//! The family registry and the FAM-38 contracts.

use ising_generators::descriptor::ValueType;
use ising_generators::descriptor::{check_family_id, Descriptor, IdContext};
use quip_solver_conformance::families::reference_revision;
use quip_solver_conformance::families::registry::{
    parse_registry, unregistered_quip_ids, RegistryEntry, Status,
};
use quip_solver_conformance::families::REFERENCE_REVISIONS;
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

/// A FAM-38 contract document: the title and the level-2 items in file order.
struct ContractDoc {
    title: String,
    items: Vec<(String, String)>,
}

/// One row of a Parameter schema table.
struct SchemaRow {
    tag: u16,
    name: String,
    kind: String,
    scale: Option<u32>,
    min: i128,
    max: i128,
}

const FAM_38_ITEMS: [&str; 13] = [
    "ID",
    "Revision",
    "Status",
    "Parameter schema",
    "Substream labels",
    "Construction steps",
    "Output order",
    "Offset",
    "Metadata",
    "Constraint class",
    "Bounds",
    "Size bounds",
    "Max retries",
];

const SCHEMA_TYPES: [&str; 9] = [
    "u8", "u16", "u32", "u64", "i32", "i64", "bool", "bytes", "ascii",
];

fn read_text(path: &str) -> Result<String, String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read_to_string(root.join(path)).map_err(|e| format!("{path}: {e}"))
}

fn parse_contract(text: &str) -> Result<ContractDoc, String> {
    let mut title = None;
    let mut items: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            items.push((name.trim().to_owned(), String::new()));
        } else if let Some(heading) = line.strip_prefix("# ") {
            if title.replace(heading.trim().to_owned()).is_some() {
                return Err("more than one title".to_owned());
            }
        } else if let Some((_, body)) = items.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    Ok(ContractDoc {
        title: title.ok_or("no title")?,
        items,
    })
}

fn read_contract(path: &str) -> Result<ContractDoc, String> {
    parse_contract(&read_text(path)?)
}

fn item<'a>(doc: &'a ContractDoc, name: &str) -> Result<&'a str, String> {
    doc.items
        .iter()
        .find(|(heading, _)| heading == name)
        .map(|(_, body)| body.as_str())
        .ok_or_else(|| format!("no item {name}"))
}

fn first_value(doc: &ContractDoc, name: &str) -> Result<String, String> {
    let line = item(doc, name)?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| format!("{name}: empty"))?;
    line.strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .filter(|value| !value.is_empty() && !value.contains('`'))
        .map(str::to_owned)
        .ok_or_else(|| format!("{name}: first line is not one backticked value: {line}"))
}

fn cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    inner
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

/// The data rows of the first table in `body`, after a header check.
fn table(body: &str, header: &[&str]) -> Result<Vec<Vec<String>>, String> {
    let mut lines = body
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'));
    let head = cells(lines.next().ok_or("no table")?);
    if head != header {
        return Err(format!("table header {head:?} != {header:?}"));
    }
    let mut rows = Vec::new();
    for line in lines {
        let row = cells(line);
        if row
            .iter()
            .all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-'))
        {
            continue;
        }
        if row.len() != header.len() {
            return Err(format!("row {row:?} has {} cells", row.len()));
        }
        rows.push(row);
    }
    Ok(rows)
}

fn schema_rows(doc: &ContractDoc) -> Result<Vec<SchemaRow>, String> {
    let rows = table(
        item(doc, "Parameter schema")?,
        &["Tag", "Name", "Type", "Scale", "Min", "Max"],
    )?;
    rows.iter()
        .map(|row| {
            if row.iter().any(String::is_empty) {
                return Err(format!("empty cell in {row:?}"));
            }
            let [tag, name, kind, scale, min, max] = row.as_slice() else {
                return Err(format!("bad row {row:?}"));
            };
            let scale = if scale == "none" {
                None
            } else {
                Some(
                    scale
                        .parse::<u32>()
                        .map_err(|e| format!("scale {scale}: {e}"))?,
                )
            };
            Ok(SchemaRow {
                tag: tag.parse().map_err(|e| format!("tag {tag}: {e}"))?,
                name: name.clone(),
                kind: kind.clone(),
                scale,
                min: min.parse().map_err(|e| format!("min {min}: {e}"))?,
                max: max.parse().map_err(|e| format!("max {max}: {e}"))?,
            })
        })
        .collect()
}

/// SC-7 #13: the FAM-38 items, in order, each once, none empty.
fn check_items(doc: &ContractDoc) -> Result<(), String> {
    let names: Vec<&str> = doc.items.iter().map(|(name, _)| name.as_str()).collect();
    if names != FAM_38_ITEMS {
        return Err(format!("items {names:?} != {FAM_38_ITEMS:?}"));
    }
    for (name, body) in &doc.items {
        if body.trim().is_empty() {
            return Err(format!("item {name} is empty"));
        }
    }
    Ok(())
}

/// SC-7 #5 and #6.
fn check_schema(rows: &[SchemaRow]) -> Result<(), String> {
    let mut previous: Option<u16> = None;
    for row in rows {
        let name = &row.name;
        if name.is_empty() {
            return Err(format!("tag {}: no name", row.tag));
        }
        if !SCHEMA_TYPES.contains(&row.kind.as_str()) {
            return Err(format!(
                "{name}: type {} is not allowed (FAM-13, FAM-14)",
                row.kind
            ));
        }
        if previous.is_some_and(|tag| row.tag <= tag) {
            return Err(format!("{name}: tags do not rise strictly"));
        }
        previous = Some(row.tag);
        let unscaled = matches!(row.kind.as_str(), "bool" | "bytes" | "ascii");
        match row.scale {
            Some(_) if unscaled => return Err(format!("{name}: a {} has no scale", row.kind)),
            None if !unscaled => return Err(format!("{name}: no scale (FAM-15)")),
            Some(0) => return Err(format!("{name}: scale is zero")),
            _ => {}
        }
        if row.min > row.max {
            return Err(format!("{name}: min above max"));
        }
        if row.kind == "bool" && (row.min, row.max) != (0, 1) {
            return Err(format!("{name}: a bool has bounds 0 and 1"));
        }
        if matches!(row.kind.as_str(), "bytes" | "ascii") && row.min < 0 {
            return Err(format!("{name}: a length minimum is below 0"));
        }
    }
    Ok(())
}

/// SC-7 #9.
fn check_max_retries(doc: &ContractDoc) -> Result<(), String> {
    if first_value(doc, "Max retries").is_ok_and(|value| value == "none") {
        return Ok(());
    }
    let rows = table(item(doc, "Max retries")?, &["Step", "Max tries"])?;
    if rows.is_empty() {
        return Err("Max retries: table has no rows".to_owned());
    }
    for row in &rows {
        let tries = row.get(1).ok_or("row without tries")?;
        if tries.parse::<u32>().map_or(true, |n| n == 0) {
            return Err(format!("Max retries: {tries} is not a positive integer"));
        }
    }
    Ok(())
}

/// SC-7 #15.
fn check_constraint_class(doc: &ContractDoc) -> Result<(), String> {
    match first_value(doc, "Constraint class")?.as_str() {
        "soft" => Ok(()),
        "hard" => {
            let rows = table(item(doc, "Constraint class")?, &["Constraint", "Meaning"])?;
            if rows.is_empty() {
                return Err("a hard class lists no constraint".to_owned());
            }
            Ok(())
        }
        other => Err(format!("class {other} is not soft or hard")),
    }
}

/// SC-7 #17 and #42.
fn check_adapted(
    entries: &[RegistryEntry],
    class_of: &dyn Fn(&RegistryEntry) -> Result<String, String>,
) -> Result<(), String> {
    for entry in entries {
        let Some(original) = &entry.adapted_from else {
            continue;
        };
        let id = &entry.family_id;
        if original == id {
            return Err(format!("{id}: adapted from itself"));
        }
        if entries
            .iter()
            .any(|e| e.adapted_from.is_none() && e.family_id == *id)
        {
            return Err(format!(
                "{id}: shares its id with an original family (FAM-41)"
            ));
        }
        let class = class_of(entry)?;
        if class != "hard" {
            return Err(format!("{id}: adapted family has class {class} (FAM-41)"));
        }
        if entry.status != Status::Research {
            return Err(format!("{id}: adapted family is not research (FAM-77)"));
        }
    }
    Ok(())
}

fn status_name(status: Status) -> &'static str {
    match status {
        Status::Research => "research",
        Status::Archived => "archived",
        Status::ChainAuthorized => "chain-authorized",
        _ => "unknown",
    }
}

fn check_status(doc: &ContractDoc, entry: &RegistryEntry) -> Result<(), String> {
    let status = first_value(doc, "Status")?;
    if status != status_name(entry.status) {
        return Err(format!(
            "{}: contract status {status} != registry status {}",
            entry.family_id,
            status_name(entry.status)
        ));
    }
    Ok(())
}

fn contracts() -> Result<Vec<(RegistryEntry, ContractDoc)>, String> {
    registry()?
        .into_iter()
        .map(|entry| {
            let doc = read_contract(&entry.contract)?;
            Ok((entry, doc))
        })
        .collect()
}

fn expect_err(result: Result<(), String>, what: &str) -> Result<(), String> {
    result.map_or_else(|_| Ok(()), |()| Err(format!("{what} passed")))
}

fn type_name(kind: ValueType) -> &'static str {
    match kind {
        ValueType::U8 => "u8",
        ValueType::U16 => "u16",
        ValueType::U32 => "u32",
        ValueType::U64 => "u64",
        ValueType::I32 => "i32",
        ValueType::I64 => "i64",
        ValueType::Bool => "bool",
        ValueType::Bytes => "bytes",
        ValueType::Ascii => "ascii",
    }
}

/// A registry JSON with one entry per `(id, status, adapted_from)`.
fn synthetic(entries: &[(&str, &str, Option<&str>)]) -> Result<Vec<RegistryEntry>, String> {
    let pin = "0".repeat(64);
    let body: Vec<String> = entries
        .iter()
        .map(|(id, status, adapted)| {
            let adapted = adapted.map_or("null".to_owned(), |a| format!("\"{a}\""));
            format!(
                "{{\"family_id\":\"{id}\",\"revision\":1,\"status\":\"{status}\",\
                 \"adapted_from\":{adapted},\"contract\":\"c.md\",\"vectors\":\"v.json\",\
                 \"vectors_blake3\":\"{pin}\"}}"
            )
        })
        .collect();
    parse_registry(&format!("{{\"registry\":[{}]}}", body.join(",")))
}

#[test]
fn every_contract_has_every_fam_38_item() -> Result<(), String> {
    for (entry, doc) in contracts()? {
        check_items(&doc).map_err(|e| format!("{}: {e}", entry.family_id))?;
    }
    Ok(())
}

#[test]
fn contract_ids_and_revisions_match_the_registry() -> Result<(), String> {
    for (entry, doc) in contracts()? {
        let (id, revision) = (&entry.family_id, entry.revision);
        if first_value(&doc, "ID")? != *id {
            return Err(format!("{id}: ID item differs"));
        }
        if first_value(&doc, "Revision")? != revision.to_string() {
            return Err(format!("{id}: Revision item differs"));
        }
        if doc.title != format!("{id} revision {revision}") {
            return Err(format!("{id}: title {:?}", doc.title));
        }
    }
    Ok(())
}

#[test]
fn contract_status_equals_the_registry_status() -> Result<(), String> {
    for (entry, doc) in contracts()? {
        check_status(&doc, &entry)?;
    }
    Ok(())
}

#[test]
fn parameter_schemas_use_no_float_type() -> Result<(), String> {
    for (entry, doc) in contracts()? {
        for row in schema_rows(&doc)? {
            if row.kind.starts_with('f') {
                return Err(format!("{}: float type {}", entry.family_id, row.kind));
            }
        }
    }
    Ok(())
}

#[test]
fn parameter_schemas_list_tag_name_type_scale_bounds() -> Result<(), String> {
    for (entry, doc) in contracts()? {
        let rows = schema_rows(&doc)?;
        if rows.is_empty() {
            return Err(format!("{}: empty schema", entry.family_id));
        }
        check_schema(&rows).map_err(|e| format!("{}: {e}", entry.family_id))?;
    }
    Ok(())
}

#[test]
fn contract_tables_agree_with_the_reference_contracts() -> Result<(), String> {
    for (entry, doc) in contracts()? {
        let id = &entry.family_id;
        let reference = reference_revision(id, entry.revision)
            .ok_or_else(|| format!("{id}: no reference revision"))?
            .contract;
        let documented: Vec<(u16, String, i128, i128)> = schema_rows(&doc)?
            .into_iter()
            .map(|r| (r.tag, r.kind, r.min, r.max))
            .collect();
        let coded: Vec<(u16, String, i128, i128)> = reference
            .schema
            .iter()
            .map(|f| (f.tag, type_name(f.kind).to_owned(), f.min, f.max))
            .collect();
        if documented != coded {
            return Err(format!("{id}: schema {documented:?} != {coded:?}"));
        }
        let labels: Vec<String> = table(item(&doc, "Substream labels")?, &["Label", "Draws"])?
            .into_iter()
            .map(|row| row.first().cloned().unwrap_or_default())
            .collect();
        let construction = reference.construction.ok_or("no construction")?;
        if labels != construction.labels {
            return Err(format!(
                "{id}: labels {labels:?} != {:?}",
                construction.labels
            ));
        }
        let kinds = table(item(&doc, "Metadata")?, &["Kind", "Name", "Payload"])?
            .into_iter()
            .map(|row| {
                let kind = row.first().cloned().unwrap_or_default();
                kind.parse::<u16>().map_err(|e| format!("kind {kind}: {e}"))
            })
            .collect::<Result<Vec<u16>, String>>()?;
        if kinds != reference.metadata_kinds {
            return Err(format!(
                "{id}: kinds {kinds:?} != {:?}",
                reference.metadata_kinds
            ));
        }
    }
    Ok(())
}

#[test]
fn contracts_state_max_retries() -> Result<(), String> {
    for (entry, doc) in contracts()? {
        check_max_retries(&doc).map_err(|e| format!("{}: {e}", entry.family_id))?;
    }
    Ok(())
}

#[test]
fn contracts_declare_a_constraint_class() -> Result<(), String> {
    for (entry, doc) in contracts()? {
        check_constraint_class(&doc).map_err(|e| format!("{}: {e}", entry.family_id))?;
    }
    Ok(())
}

fn contract_class(entry: &RegistryEntry) -> Result<String, String> {
    first_value(&read_contract(&entry.contract)?, "Constraint class")
}

#[test]
fn adapted_families_are_hard_with_separate_ids() -> Result<(), String> {
    check_adapted(&registry()?, &contract_class)
}

#[test]
fn adapted_families_have_research_status() -> Result<(), String> {
    check_adapted(&registry()?, &contract_class)?;
    for entry in registry()? {
        if entry.adapted_from.is_some() && entry.status != Status::Research {
            return Err(format!("{}: adapted and not research", entry.family_id));
        }
    }
    Ok(())
}

#[test]
fn quip_ids_in_vectors_and_contracts_are_registered() -> Result<(), String> {
    let registry = registry()?;
    let mut ids: Vec<String> = registry.iter().map(|e| e.family_id.clone()).collect();
    for (_, doc) in contracts()? {
        ids.push(first_value(&doc, "ID")?);
    }
    let layered: Value = serde_json::from_str(FAMILY_LAYERED).map_err(|e| e.to_string())?;
    for case in layered_cases(&layered)? {
        ids.push(case_key(case)?.0);
    }
    for reference in REFERENCE_REVISIONS {
        ids.push(reference.contract.family_id.to_owned());
    }
    let missing = unregistered_quip_ids(&registry, ids.iter().map(String::as_str));
    if !missing.is_empty() {
        return Err(format!("unregistered quip ids: {missing:?}"));
    }
    Ok(())
}

#[test]
fn a_contract_missing_an_item_fails() -> Result<(), String> {
    let text = read_text("docs/families/vectors-uniform-field-1.md")?;
    let doc = parse_contract(&text.replace("## Max retries\n", ""))?;
    expect_err(check_items(&doc), "a contract without Max retries")
}

fn row(kind: &str, scale: Option<u32>, min: i128, max: i128) -> SchemaRow {
    SchemaRow {
        tag: 1,
        name: "x".to_owned(),
        kind: kind.to_owned(),
        scale,
        min,
        max,
    }
}

#[test]
fn a_float_schema_type_fails() -> Result<(), String> {
    expect_err(check_schema(&[row("f64", Some(1), 0, 1)]), "an f64 field")
}

#[test]
fn a_bool_field_with_a_scale_fails() -> Result<(), String> {
    check_schema(&[row("bool", None, 0, 1)])?;
    expect_err(check_schema(&[row("bool", Some(1), 0, 1)]), "a scaled bool")
}

#[test]
fn a_hard_contract_without_constraints_fails() -> Result<(), String> {
    let doc = ContractDoc {
        title: "t".to_owned(),
        items: vec![("Constraint class".to_owned(), "`hard`\n".to_owned())],
    };
    expect_err(
        check_constraint_class(&doc),
        "a hard class without constraints",
    )
}

#[test]
fn an_adapted_soft_family_fails() -> Result<(), String> {
    let entries = synthetic(&[("vectors/a", "research", Some("vectors/b"))])?;
    check_adapted(&entries, &|_| Ok("hard".to_owned()))?;
    expect_err(
        check_adapted(&entries, &|_| Ok("soft".to_owned())),
        "an adapted soft family",
    )
}

#[test]
fn an_adapted_family_sharing_an_original_id_fails() -> Result<(), String> {
    let entries = synthetic(&[
        ("vectors/a", "research", Some("vectors/b")),
        ("vectors/a", "research", None),
    ])?;
    expect_err(
        check_adapted(&entries, &|_| Ok("hard".to_owned())),
        "a shared id",
    )?;
    let itself = synthetic(&[("vectors/a", "research", Some("vectors/a"))])?;
    expect_err(
        check_adapted(&itself, &|_| Ok("hard".to_owned())),
        "an adapted-from-itself family",
    )
}

#[test]
fn an_adapted_family_outside_research_fails() -> Result<(), String> {
    let entries = synthetic(&[("vectors/a", "chain-authorized", Some("vectors/b"))])?;
    expect_err(
        check_adapted(&entries, &|_| Ok("hard".to_owned())),
        "an adapted family outside research",
    )
}

#[test]
fn a_contract_status_that_differs_from_the_registry_fails() -> Result<(), String> {
    let text = read_text("docs/families/vectors-uniform-field-1.md")?;
    let doc = parse_contract(&text.replace("## Status\n\n`research`", "## Status\n\n`archived`"))?;
    let entry = registry()?
        .into_iter()
        .find(|e| e.family_id == "vectors/uniform-field")
        .ok_or("no entry")?;
    expect_err(check_status(&doc, &entry), "a differing status")
}
