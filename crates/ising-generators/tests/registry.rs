//! The contract registry lists every implemented family revision, sorted and
//! without duplicates.

#![expect(
    clippy::expect_used,
    reason = "test code: a failed lookup is a test failure"
)]

use std::fs;
use std::path::Path;

use ising_generators::registry::CONTRACTS;

/// The names of the `pub const NAME_V<n>: Contract` items in `text`.
fn contract_names(text: &str) -> Vec<String> {
    // Whitespace and newlines may separate the parts of a declaration.
    let words: Vec<&str> = text
        .split(|c: char| c.is_whitespace() || c == ':')
        .filter(|w| !w.is_empty())
        .collect();
    let mut names = Vec::new();
    for window in words.windows(4) {
        if let [first, second, name, "Contract"] = *window {
            if first == "pub" && second == "const" && is_revision_name(name) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// Whether `name` ends in `_V` and digits.
fn is_revision_name(name: &str) -> bool {
    name.rsplit_once("_V").is_some_and(|(stem, rev)| {
        !stem.is_empty() && !rev.is_empty() && rev.bytes().all(|b| b.is_ascii_digit())
    })
}

/// The contract constants of the crate sources, found by scanning `src`
/// independently of the registry.
fn contract_constants() -> Vec<String> {
    fn walk(dir: &Path, found: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("read src directory") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = fs::read_to_string(&path).expect("read source file");
                found.extend(contract_names(&text));
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

#[test]
fn the_scan_finds_split_declarations_and_later_revisions() {
    let text = "pub const A_V1: Contract = X;\npub const\n  B_V12:\n    Contract = Y;\n\
                pub const C: Contract = Z;\npub const D_V1: u8 = 0;";
    assert_eq!(contract_names(text), ["A_V1", "B_V12"]);
}
