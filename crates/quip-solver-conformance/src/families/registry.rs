//! The family registry (FAM-09): one entry per family revision, with its
//! status, adapted-family marker, contract, and frozen vectors (FAM-08).

use serde_json::Value;

/// The status of a family revision (FAM-09).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// The revision is under study and no chain accepts it.
    Research,
    /// The revision is retired and generated families must not list it.
    Archived,
    /// A chain upgrade authorized the revision.
    ChainAuthorized,
}

/// One family revision in the registry.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RegistryEntry {
    /// The family id.
    pub family_id: String,
    /// The revision, at least 1.
    pub revision: u32,
    /// The status of the revision.
    pub status: Status,
    /// The `family_id` of the original family, for an adapted family (FAM-41, FAM-77).
    pub adapted_from: Option<String>,
    /// Repository-relative path of the FAM-38 contract.
    pub contract: String,
    /// Repository-relative path of the vectors file.
    pub vectors: String,
    /// BLAKE3 of the canonical JSON of the revision's cases in the vectors file.
    pub vectors_blake3: [u8; 32],
}

fn field<'a>(entry: &'a Value, path: &str, key: &str) -> Result<&'a Value, String> {
    entry
        .get(key)
        .ok_or_else(|| format!("{path}.{key}: missing field"))
}

fn text<'a>(entry: &'a Value, path: &str, key: &str) -> Result<&'a str, String> {
    field(entry, path, key)?
        .as_str()
        .ok_or_else(|| format!("{path}.{key}: not a string"))
}

fn parse_status(name: &str, path: &str) -> Result<Status, String> {
    match name {
        "research" => Ok(Status::Research),
        "archived" => Ok(Status::Archived),
        "chain-authorized" => Ok(Status::ChainAuthorized),
        other => Err(format!("{path}.status: unknown status {other:?}")),
    }
}

fn parse_pin(hex: &str, path: &str) -> Result<[u8; 32], String> {
    let bad = || format!("{path}.vectors_blake3: need 64 lowercase hex characters");
    if hex.len() != 64 {
        return Err(bad());
    }
    let mut pin = [0_u8; 32];
    for (byte, pair) in pin.iter_mut().zip(hex.as_bytes().chunks(2)) {
        let digit = |c: u8| match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            _ => Err(bad()),
        };
        let [high, low] = pair else {
            return Err(bad());
        };
        *byte = digit(*high)? << 4 | digit(*low)?;
    }
    Ok(pin)
}

fn parse_entry(entry: &Value, path: &str) -> Result<RegistryEntry, String> {
    let revision = field(entry, path, "revision")?
        .as_u64()
        .and_then(|r| u32::try_from(r).ok())
        .ok_or_else(|| format!("{path}.revision: not a u32"))?;
    let adapted_from = match field(entry, path, "adapted_from")? {
        Value::Null => None,
        Value::String(id) => Some(id.clone()),
        _ => return Err(format!("{path}.adapted_from: not a string or null")),
    };
    Ok(RegistryEntry {
        family_id: text(entry, path, "family_id")?.to_owned(),
        revision,
        status: parse_status(text(entry, path, "status")?, path)?,
        adapted_from,
        contract: text(entry, path, "contract")?.to_owned(),
        vectors: text(entry, path, "vectors")?.to_owned(),
        vectors_blake3: parse_pin(text(entry, path, "vectors_blake3")?, path)?,
    })
}

/// Parse the `registry` section of a file such as `FAMILY_REGISTRY`.
///
/// # Errors
///
/// A missing or malformed field, named by its JSON path.
pub fn parse_registry(json: &str) -> Result<Vec<RegistryEntry>, String> {
    let doc: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    doc.get("registry")
        .ok_or_else(|| "registry: missing section".to_owned())?
        .as_array()
        .ok_or_else(|| "registry: not an array".to_owned())?
        .iter()
        .enumerate()
        .map(|(index, entry)| parse_entry(entry, &format!("registry[{index}]")))
        .collect()
}

/// FAM-04: the `quip/` ids in `ids` that no entry registers, sorted, without repeats.
#[must_use]
pub fn unregistered_quip_ids<'a>(
    registry: &[RegistryEntry],
    ids: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut missing: Vec<String> = ids
        .into_iter()
        .filter(|id| id.starts_with("quip/"))
        .filter(|id| registry.iter().all(|entry| entry.family_id != *id))
        .map(str::to_owned)
        .collect();
    missing.sort();
    missing.dedup();
    missing
}

/// FAM-59: the pairs in `generated` that the registry lists as `archived`.
#[must_use]
pub fn archived_among(
    registry: &[RegistryEntry],
    generated: &[(String, u32)],
) -> Vec<(String, u32)> {
    generated
        .iter()
        .filter(|(id, revision)| {
            registry.iter().any(|entry| {
                entry.status == Status::Archived
                    && entry.family_id == *id
                    && entry.revision == *revision
            })
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "d9b3a89d5ad120221c51691faf44edaa7fb699c2b809c9aeadecd793dfe6cc08";

    fn doc(status: &str, pin: &str) -> String {
        format!(
            r#"{{"registry": [{{"adapted_from": null, "contract": "c.md", "evidence": [],
            "family_id": "quip/x", "name": "quip/x@2", "revision": 2, "status": "{status}",
            "vectors": "v.json", "vectors_blake3": "{pin}"}}]}}"#
        )
    }

    #[test]
    fn parse_rejects_an_unknown_status() {
        let err = parse_registry(&doc("draft", PIN));
        assert_eq!(
            err,
            Err("registry[0].status: unknown status \"draft\"".to_owned())
        );
    }

    #[test]
    fn parse_rejects_a_short_pin() {
        let err = parse_registry(&doc("research", &PIN[..62]));
        assert_eq!(
            err,
            Err("registry[0].vectors_blake3: need 64 lowercase hex characters".to_owned())
        );
    }

    #[test]
    fn an_unregistered_quip_id_is_reported() {
        let missing = unregistered_quip_ids(&[], ["quip/ice", "vectors/x", "quip/ice"]);
        assert_eq!(missing, ["quip/ice"]);
    }

    #[test]
    fn an_archived_generated_revision_is_reported() {
        let generated = [("quip/x".to_owned(), 1), ("quip/x".to_owned(), 2)];
        let archived = parse_registry(&doc("archived", PIN));
        assert_eq!(
            archived.map(|r| archived_among(&r, &generated)),
            Ok(vec![("quip/x".to_owned(), 2)])
        );
        let research = parse_registry(&doc("research", PIN));
        assert_eq!(research.map(|r| archived_among(&r, &generated)), Ok(vec![]));
    }
}
