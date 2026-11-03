//! `quip-topology`: converts between topology JSON and `.bin.gz` blobs.
//!
//! The subcommands, JSON shapes, and exit codes match
//! `python -m family_oracle topology`, so either tool reads what the other
//! wrote. Exit 0 is success, 1 is a format or file error (the `BlobError` name
//! goes to stderr), and 2 is a usage error.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;

use ising_generators::blob::{decode, encode, BlobError, Body, Topology};
use serde_json::Value;

const USAGE: &str = "usage: quip-topology encode IN.json OUT.bin.gz\n       \
                     quip-topology decode [--expand] IN.bin.gz [OUT.json]";

/// Why a command failed, apart from a usage error.
enum Failure {
    Blob(BlobError),
    Message(String),
}

impl From<BlobError> for Failure {
    fn from(error: BlobError) -> Self {
        Self::Blob(error)
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::Message(error.to_string())
    }
}

fn shape(what: &str) -> Failure {
    Failure::Message(format!("invalid JSON: {what}"))
}

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let expand = args.iter().any(|arg| arg == "--expand");
    let args: Vec<&OsStr> = args
        .iter()
        .map(OsString::as_os_str)
        .filter(|arg| *arg != "--expand")
        .collect();
    let result = match (args.as_slice(), expand) {
        ([command, input, output], false) if *command == "encode" => {
            run_encode(Path::new(input), Path::new(output))
        }
        ([command, input], _) if *command == "decode" => run_decode(Path::new(input), None, expand),
        ([command, input, output], _) if *command == "decode" => {
            run_decode(Path::new(input), Some(Path::new(output)), expand)
        }
        _ => {
            let _ = writeln!(std::io::stderr(), "{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            let text = match failure {
                Failure::Blob(error) => format!("{error:?}"),
                Failure::Message(message) => message,
            };
            let _ = writeln!(std::io::stderr(), "topology: {text}");
            ExitCode::from(1)
        }
    }
}

fn run_encode(input: &Path, output: &Path) -> Result<(), Failure> {
    let value: Value = serde_json::from_slice(&std::fs::read(input)?)
        .map_err(|error| Failure::Message(format!("invalid JSON: {error}")))?;
    let body = body_from_json(&value)?;
    std::fs::write(output, encode(&body)?)?;
    Ok(())
}

fn run_decode(input: &Path, output: Option<&Path>, expand: bool) -> Result<(), Failure> {
    let body = decode(&std::fs::read(input)?)?;
    let text = if expand {
        render_explicit(&body.topology()?)
    } else {
        render(&body)
    };
    match output {
        Some(path) => std::fs::write(path, text)?,
        None => std::io::stdout().write_all(text.as_bytes())?,
    }
    Ok(())
}

// --- JSON in ---------------------------------------------------------------

fn id(value: &Value) -> Result<u32, Failure> {
    let number = value
        .as_i64()
        .ok_or_else(|| shape("an id is not an integer"))?;
    u32::try_from(number).map_err(|_| Failure::Blob(BlobError::Range))
}

fn ids(value: Option<&Value>) -> Result<Vec<u32>, Failure> {
    let list = value
        .and_then(Value::as_array)
        .ok_or_else(|| shape("expected an array of ids"))?;
    list.iter().map(id).collect()
}

fn pairs(value: Option<&Value>) -> Result<Vec<(u32, u32)>, Failure> {
    let list = value
        .and_then(Value::as_array)
        .ok_or_else(|| shape("expected an array of edges"))?;
    list.iter()
        .map(|edge| match edge.as_array().map(Vec::as_slice) {
            Some([u, v]) => Ok((id(u)?, id(v)?)),
            _ => Err(shape("an edge is not a pair")),
        })
        .collect()
}

fn small(value: Option<&Value>) -> Result<u64, Failure> {
    value
        .and_then(Value::as_u64)
        .ok_or_else(|| shape("m and t must be non-negative integers"))
}

fn body_from_json(value: &Value) -> Result<Body, Failure> {
    match value.get("kind").and_then(Value::as_str) {
        Some("explicit") => Ok(Body::Explicit(Topology {
            nodes: ids(value.get("nodes"))?,
            edges: pairs(value.get("edges"))?,
        })),
        Some("zephyr-removed") => Ok(Body::ZephyrRemoved {
            m: u16::try_from(small(value.get("m"))?).map_err(|_| BlobError::Range)?,
            t: u8::try_from(small(value.get("t"))?).map_err(|_| BlobError::Range)?,
            removed_nodes: ids(value.get("removed_nodes"))?,
            removed_edges: pairs(value.get("removed_edges"))?,
        }),
        _ => Err(Failure::Blob(BlobError::Kind)),
    }
}

// --- JSON out: the layout of json.dumps(indent=2, sort_keys=True) ----------

fn int_list(out: &mut String, ids: &[u32]) {
    if ids.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[\n");
    for (index, id) in ids.iter().enumerate() {
        let comma = if index + 1 < ids.len() { "," } else { "" };
        let _ = writeln!(out, "    {id}{comma}");
    }
    out.push_str("  ]");
}

fn pair_list(out: &mut String, pairs: &[(u32, u32)]) {
    if pairs.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[\n");
    for (index, (u, v)) in pairs.iter().enumerate() {
        let comma = if index + 1 < pairs.len() { "," } else { "" };
        let _ = writeln!(out, "    [\n      {u},\n      {v}\n    ]{comma}");
    }
    out.push_str("  ]");
}

fn render_explicit(topology: &Topology) -> String {
    let mut out = String::from("{\n  \"edges\": ");
    pair_list(&mut out, &topology.edges);
    out.push_str(",\n  \"kind\": \"explicit\",\n  \"nodes\": ");
    int_list(&mut out, &topology.nodes);
    out.push_str("\n}\n");
    out
}

fn render(body: &Body) -> String {
    match body {
        Body::Explicit(topology) => render_explicit(topology),
        Body::ZephyrRemoved {
            m,
            t,
            removed_nodes,
            removed_edges,
        } => {
            let mut out = String::from("{\n  \"kind\": \"zephyr-removed\",\n");
            let _ = write!(out, "  \"m\": {m},\n  \"removed_edges\": ");
            pair_list(&mut out, removed_edges);
            out.push_str(",\n  \"removed_nodes\": ");
            int_list(&mut out, removed_nodes);
            let _ = write!(out, ",\n  \"t\": {t}\n}}\n");
            out
        }
    }
}
