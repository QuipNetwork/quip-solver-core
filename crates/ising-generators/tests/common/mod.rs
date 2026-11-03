//! Shared test loaders for the committed ice fixtures: the live topology, the
//! archived tiles, and small hex helpers.
//!
//! The archived tiles live in `quip-solver-conformance/vectors/families/ice/`.
//! They are read with `include_str!` and `include_bytes!`, so a test never
//! touches the file system at run time. The live topology comes from the
//! topology files that `quip_solver_conformance::vectors` embeds.

use ising_generators::family::BaseTopology;
use serde_json::Value;

/// The reference to the live topology file, as a vectors file holds it.
pub(crate) const LIVE_REFERENCE: &str = "../topologies/zephyr-z12t4-live.bin.gz";

/// The sidecar of the archived tiles: provenance, digest, and per-model offsets.
pub(crate) const ARCHIVED_JSON: &str =
    include_str!("../../../quip-solver-conformance/vectors/families/ice/archived-tiles.json");

/// The archived tiles: `u16` little-endian dense positions, four per tile.
pub(crate) const ARCHIVED_BIN: &[u8] =
    include_bytes!("../../../quip-solver-conformance/vectors/families/ice/archived-tiles.bin");

/// The structure vectors (`ice_structure.json`).
pub(crate) const STRUCTURE_JSON: &str =
    include_str!("../../../quip-solver-conformance/vectors/families/ice_structure.json");

/// Parse a fixture string.
///
/// # Panics
///
/// When the text is not JSON.
pub(crate) fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture is valid JSON")
}

/// The field `key` of a JSON object.
///
/// # Panics
///
/// When the field is missing.
pub(crate) fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value
        .get(key)
        .unwrap_or_else(|| panic!("missing field {key}"))
}

/// The field `key` as an array.
///
/// # Panics
///
/// When the field is missing or not an array.
pub(crate) fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    field(value, key)
        .as_array()
        .unwrap_or_else(|| panic!("{key} is not an array"))
}

/// The field `key` as a string.
///
/// # Panics
///
/// When the field is missing or not a string.
pub(crate) fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    field(value, key)
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not a string"))
}

/// The field `key` as a `u64`.
///
/// # Panics
///
/// When the field is missing or not an unsigned integer.
pub(crate) fn number(value: &Value, key: &str) -> u64 {
    field(value, key)
        .as_u64()
        .unwrap_or_else(|| panic!("{key} is not an unsigned integer"))
}

/// A JSON array of unsigned integers as `u32` values.
///
/// # Panics
///
/// When an element is not a `u32`.
pub(crate) fn u32s(value: &Value) -> Vec<u32> {
    value
        .as_array()
        .expect("an array")
        .iter()
        .map(|v| u32::try_from(v.as_u64().expect("an integer")).expect("fits u32"))
        .collect()
}

/// A JSON array of integer pairs.
///
/// # Panics
///
/// When an element is not a pair of `u32` values.
pub(crate) fn pairs(value: &Value) -> Vec<(u32, u32)> {
    value
        .as_array()
        .expect("an array")
        .iter()
        .map(|pair| {
            let two = u32s(pair);
            assert_eq!(two.len(), 2, "a pair");
            (two[0], two[1])
        })
        .collect()
}

/// Lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// The bytes of a lowercase hex string.
///
/// # Panics
///
/// When the string has an odd length or a character that is not hex, or
/// when a byte offset does not fall on a character boundary.
pub(crate) fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0, "even hex length");
    (0..text.len())
        .step_by(2)
        .map(|at| {
            let pair = text
                .get(at..at + 2)
                .expect("hex pair on a character boundary");
            u8::from_str_radix(pair, 16).expect("hex")
        })
        .collect()
}

/// The 32-byte seed in a hex string.
///
/// # Panics
///
/// When the string is not 64 hex characters.
pub(crate) fn seed(text: &str) -> [u8; 32] {
    unhex(text).try_into().expect("a 32-byte seed")
}

/// The index entry of the live topology: counts, order digest, provenance.
///
/// # Panics
///
/// When the index has no live entry.
pub(crate) fn live_entry() -> Value {
    let index = parse(quip_solver_conformance::vectors::TOPOLOGY_INDEX);
    index
        .as_array()
        .expect("the index is an array")
        .iter()
        .find(|entry| text(entry, "file") == "zephyr-z12t4-live.bin.gz")
        .expect("the index lists the live topology")
        .clone()
}

/// The live z(12, 4) topology, decoded from its file (kind 1).
///
/// # Panics
///
/// When the file does not decode or the topology is invalid.
pub(crate) fn live_topology() -> BaseTopology {
    let stored = quip_solver_conformance::vectors::topology(LIVE_REFERENCE).expect("live file");
    BaseTopology::new(stored.nodes, &stored.edges).expect("live topology")
}

/// One archived model: its nonce and its tiles in dense positions.
pub(crate) struct ArchivedModel {
    /// The nonce in lowercase hex.
    pub(crate) nonce: String,
    /// The tiles in archive order, four dense positions each.
    pub(crate) tiles: Vec<[u32; 4]>,
}

/// The archived models, cut from the `.bin` by the sidecar offsets.
///
/// # Panics
///
/// When an offset, a length, or a tile count does not fit the binary.
pub(crate) fn archived_models() -> Vec<ArchivedModel> {
    let sidecar = parse(ARCHIVED_JSON);
    array(&sidecar, "models")
        .iter()
        .map(|model| {
            let at = usize::try_from(number(model, "offset_bytes")).expect("offset");
            let len = usize::try_from(number(model, "length_bytes")).expect("length");
            let words: Vec<u32> = ARCHIVED_BIN[at..at + len]
                .chunks_exact(2)
                .map(|w| u32::from(u16::from_le_bytes([w[0], w[1]])))
                .collect();
            let tiles: Vec<[u32; 4]> = words
                .chunks_exact(4)
                .map(|w| [w[0], w[1], w[2], w[3]])
                .collect();
            assert_eq!(tiles.len() as u64, number(model, "tiles"), "tile count");
            ArchivedModel {
                nonce: text(model, "nonce").to_owned(),
                tiles,
            }
        })
        .collect()
}
