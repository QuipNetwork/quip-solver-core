//! The contract registry lists every implemented family revision, sorted and
//! without duplicates.

#![expect(
    clippy::expect_used,
    reason = "test code: a failed lookup is a test failure"
)]

use std::fs;
use std::path::Path;

use ising_generators::registry::CONTRACTS;

/// The names of the `pub const NAME_V1: Contract` items in the crate sources,
/// found by scanning `src` independently of the registry.
fn contract_constants() -> Vec<String> {
    fn walk(dir: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("read src directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = fs::read_to_string(&path).expect("read source file");
                for line in text.lines() {
                    let Some(rest) = line.trim().strip_prefix("pub const ") else {
                        continue;
                    };
                    let Some((name, ty)) = rest.split_once(':') else {
                        continue;
                    };
                    if name.ends_with("_V1") && ty.trim_start().starts_with("Contract") {
                        found.push(name.to_owned());
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut found,
    );
    found.sort();
    found
}

/// The registry in the order of `CONTRACTS`, paired with the constant names a
/// maintainer must list here. A new `*_V1` contract that is missing from this
/// list fails `registry_is_sorted_and_complete`.
const LISTED: [(&str, u32, &str); 8] = [
    ("quip/charged-ice", 1, "CHARGED_ICE_V1"),
    ("quip/defects-static", 1, "DEFECTS_V1"),
    ("quip/flow-static", 1, "FLOW_V1"),
    ("quip/ice-fields", 1, "ICE_FIELDS_V1"),
    ("quip/ice-glass", 1, "ICE_GLASS_V1"),
    ("quip/polariton-static", 1, "POLARITON_V1"),
    ("quip/twisted-ice-dimer", 1, "TWISTED_DIMER_V1"),
    ("quip/two-commodity-routing", 1, "ROUTING_V1"),
];

#[test]
fn registry_is_sorted_and_complete() {
    let keys: Vec<(&str, u32)> = CONTRACTS
        .iter()
        .map(|c| (c.family_id, c.revision))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(keys, sorted, "CONTRACTS must be sorted with no duplicate");

    let listed: Vec<(&str, u32)> = LISTED.iter().map(|&(id, rev, _)| (id, rev)).collect();
    assert_eq!(keys, listed, "CONTRACTS differs from the expected list");

    let mut names: Vec<String> = LISTED.iter().map(|&(_, _, name)| name.to_owned()).collect();
    names.sort();
    assert_eq!(
        contract_constants(),
        names,
        "a `*_V1` contract in src is not in the expected list, or the reverse"
    );
    assert!(CONTRACTS.iter().all(|c| c.construction.is_some()));
}
