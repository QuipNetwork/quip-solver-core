//! The `quip-topology` binary: JSON to blob to JSON, the Python-written blobs,
//! and the exit codes.

#![cfg(feature = "cli")]
#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code: a failed lookup is a test failure"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};

use ising_generators::blob::{decode, decompress};
use serde_json::{json, Value};

const T_SPARSE: &str = "tests/data/python-t-sparse.bin.gz";
const Z22: &str = "tests/data/python-z22-removed.bin.gz";

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A unique, empty directory that is removed when the guard drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "quip-topology-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(name)
}

fn run(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_quip-topology"))
        .args(args)
        .output()
        .expect("run quip-topology")
}

fn run_str(args: &[&str]) -> Output {
    let paths: Vec<&Path> = args.iter().map(Path::new).collect();
    run(&paths)
}

fn write_json(dir: &Path, name: &str, value: &Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, value.to_string()).expect("write json");
    path
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read json")).expect("parse json")
}

fn code(output: &Output) -> Option<i32> {
    output.status.code()
}

fn round_trip(dir: &Path, name: &str, value: &Value) {
    let input = write_json(dir, &format!("{name}.json"), value);
    let blob = dir.join(format!("{name}.bin.gz"));
    let back = dir.join(format!("{name}.back.json"));
    assert_eq!(code(&run(&[Path::new("encode"), &input, &blob])), Some(0));
    assert_eq!(code(&run(&[Path::new("decode"), &blob, &back])), Some(0));
    assert_eq!(read_json(&back), *value);
}

#[test]
fn explicit_round_trip_keeps_order_and_extremes() {
    let dir = Scratch::new();
    let value = json!({
        "kind": "explicit",
        "nodes": [4_294_967_295_u32, 0, 7],
        "edges": [[7, 0], [0, 0], [7, 0], [4_294_967_295_u32, 7]],
    });
    round_trip(&dir, "explicit", &value);
    let empty = json!({"kind": "explicit", "nodes": [], "edges": []});
    round_trip(&dir, "empty", &empty);
}

#[test]
fn zephyr_round_trip_and_expand() {
    let dir = Scratch::new();
    let value = json!({
        "kind": "zephyr-removed",
        "m": 1,
        "t": 1,
        "removed_nodes": [0],
        "removed_edges": [],
    });
    round_trip(&dir, "zephyr", &value);
    let blob = dir.join("zephyr.bin.gz");
    let out = run(&[Path::new("decode"), Path::new("--expand"), &blob]);
    assert_eq!(code(&out), Some(0));
    let expanded: Value = serde_json::from_slice(&out.stdout).expect("stdout is JSON");
    let body = decode(&std::fs::read(&blob).expect("read blob")).expect("decode blob");
    let topology = body.topology().expect("expand");
    let nodes: Vec<Value> = topology.nodes.iter().map(|n| json!(n)).collect();
    let edges: Vec<Value> = topology.edges.iter().map(|&(u, v)| json!([u, v])).collect();
    assert_eq!(expanded["kind"], "explicit");
    assert_eq!(expanded["nodes"], Value::Array(nodes));
    assert_eq!(expanded["edges"], Value::Array(edges));
}

#[test]
fn decodes_the_python_written_blobs() {
    let dir = Scratch::new();
    for (file, name) in [(T_SPARSE, "sparse"), (Z22, "z22")] {
        let blob = data(file);
        let json_path = dir.join(format!("{name}.json"));
        assert_eq!(
            code(&run(&[Path::new("decode"), &blob, &json_path])),
            Some(0)
        );
        // Re-encoding gives the same payload, though gzip bytes may differ.
        let again = dir.join(format!("{name}.bin.gz"));
        assert_eq!(
            code(&run(&[Path::new("encode"), &json_path, &again])),
            Some(0)
        );
        let theirs = decompress(&std::fs::read(&blob).expect("read python blob")).expect("inflate");
        let ours = decompress(&std::fs::read(&again).expect("read rust blob")).expect("inflate");
        assert_eq!(ours, theirs, "{file}");
    }
}

#[test]
fn decode_prints_sorted_two_space_json_to_stdout() {
    let out = run(&[Path::new("decode"), &data(T_SPARSE)]);
    assert_eq!(code(&out), Some(0));
    let text = String::from_utf8(out.stdout).expect("utf-8");
    assert!(text.starts_with("{\n  \"edges\": ["), "{text}");
    assert!(text.ends_with("\n}\n"));
}

#[test]
fn corrupt_file_exits_1() {
    let dir = Scratch::new();
    let mut bytes = std::fs::read(data(T_SPARSE)).expect("read");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    let bad = dir.join("bad.bin.gz");
    std::fs::write(&bad, bytes).expect("write");
    let out = run(&[Path::new("decode"), &bad]);
    assert_eq!(code(&out), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("topology: "));
    let missing = dir.join("missing.bin.gz");
    assert_eq!(code(&run(&[Path::new("decode"), &missing])), Some(1));
}

#[test]
fn bad_json_exits_1() {
    let dir = Scratch::new();
    let out_path = dir.join("out.bin.gz");
    for value in [
        json!({"kind": "nope"}),
        json!({"kind": "explicit", "nodes": [-1], "edges": []}),
        json!({"kind": "explicit", "nodes": [4_294_967_296_u64], "edges": []}),
        json!({"kind": "explicit", "nodes": [1], "edges": [[1]]}),
        json!({"kind": "zephyr-removed", "m": 99, "t": 1, "removed_nodes": [], "removed_edges": []}),
        json!([1, 2]),
    ] {
        let input = write_json(&dir, "in.json", &value);
        let out = run(&[Path::new("encode"), &input, &out_path]);
        assert_eq!(code(&out), Some(1), "{value}");
    }
}

#[test]
fn usage_errors_exit_2() {
    assert_eq!(code(&run_str(&[])), Some(2));
    assert_eq!(code(&run_str(&["encode"])), Some(2));
    assert_eq!(code(&run_str(&["encode", "in.json"])), Some(2));
    assert_eq!(code(&run_str(&["encode", "--expand", "a", "b"])), Some(2));
    assert_eq!(code(&run_str(&["decode"])), Some(2));
    assert_eq!(code(&run_str(&["frobnicate", "a"])), Some(2));
}

#[test]
fn stdout_matches_the_python_cli_byte_for_byte() {
    let cases: [(&str, &str, bool); 4] = [
        (T_SPARSE, "tests/data/python-t-sparse.json", false),
        (T_SPARSE, "tests/data/python-t-sparse.json", true),
        (Z22, "tests/data/python-z22-removed.json", false),
        (Z22, "tests/data/python-z22-removed.expanded.json", true),
    ];
    for (blob, golden, expand) in cases {
        let blob = data(blob);
        let mut args: Vec<&Path> = vec![Path::new("decode")];
        if expand {
            args.push(Path::new("--expand"));
        }
        args.push(&blob);
        let out = run(&args);
        assert_eq!(code(&out), Some(0));
        let want = std::fs::read(data(golden)).expect("read golden");
        assert_eq!(out.stdout, want, "{golden} expand={expand}");
    }
}

#[cfg(unix)]
#[test]
fn non_utf8_arguments_do_not_panic() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let bad = Path::new(OsStr::from_bytes(b"\xff"));
    assert_eq!(code(&run(&[Path::new("decode"), bad])), Some(1));
    assert_eq!(code(&run(&[bad, Path::new("x")])), Some(2));
}
