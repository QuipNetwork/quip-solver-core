//! A stand-in quantum annealer for the `--solve` gate's QPU cases.
//!
//! It lists `anneal-time` and `embed`, which no SDK-built solver can, so the
//! gate's tests have a binary to run those cases against. Its working graph is
//! every node of degree at most [`NATIVE_DEGREE`], as on a Pegasus chip. It
//! returns all `+1` reads, so the same seed gives the same samples.
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a --solve binary answers on stdout and explains refusals on stderr"
)]

use quip_protocol::scoring::energy_milli;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Read as _;
use std::process::ExitCode;

/// Highest degree on the stand-in working graph.
const NATIVE_DEGREE: usize = 15;

/// What the stand-in embedder reports as its longest chain.
const CHAIN_LENGTH: usize = 2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Problem {
    h: Vec<f64>,
    j: Vec<f64>,
    edges: Vec<(usize, usize)>,
    num_reads: usize,
    num_sweeps: usize,
    sweeps_per_beta: usize,
    beta_range: Option<(f64, f64)>,
    #[serde(default)]
    beta_schedule_type: Option<String>,
    seed: u64,
    #[serde(default)]
    anneal_time_us: Option<u64>,
}

enum Refusal {
    Malformed(String),
    Capacity(String),
}

fn check(p: &Problem, embed: bool) -> Result<(), Refusal> {
    use Refusal::{Capacity, Malformed};
    if p.j.len() != p.edges.len() {
        return Err(Malformed("j and edges differ in length".into()));
    }
    let mut degree = vec![0_usize; p.h.len()];
    for &(u, v) in &p.edges {
        if u == v {
            return Err(Malformed(format!("self-loop on {u}")));
        }
        for node in [u, v] {
            *degree
                .get_mut(node)
                .ok_or_else(|| Malformed(format!("endpoint {node} is outside h")))? += 1;
        }
    }
    if p.num_reads == 0 || p.sweeps_per_beta == 0 {
        return Err(Malformed(
            "num_reads and sweeps_per_beta must be at least 1".into(),
        ));
    }
    match p.beta_schedule_type.as_deref() {
        None | Some("geometric") => {}
        Some(other) => return Err(Malformed(format!("unsupported schedule {other}"))),
    }
    if p.anneal_time_us == Some(0) {
        return Err(Malformed("anneal_time_us must be at least 1".into()));
    }
    if !embed && degree.iter().any(|&d| d > NATIVE_DEGREE) {
        return Err(Capacity("the model is off the working graph".into()));
    }
    Ok(())
}

fn solve(input: &[u8], embed: bool) -> Result<Value, Refusal> {
    let p: Problem =
        serde_json::from_slice(input).map_err(|e| Refusal::Malformed(e.to_string()))?;
    check(&p, embed)?;
    let spins = vec![1_i8; p.h.len()];
    let mut sampler = json!({
        "backend": "dwave-qpu",
        "algorithm": "quantum-anneal",
        "num_reads": p.num_reads,
        "num_sweeps": p.num_sweeps,
        "sweeps_per_beta": p.sweeps_per_beta,
        "beta_range": p.beta_range,
        "beta_schedule_type": "geometric",
        "seed": p.seed,
    });
    if let (Some(anneal), Some(object)) = (p.anneal_time_us, sampler.as_object_mut()) {
        let _ = object.insert("anneal_time_us".into(), json!(anneal));
    }
    let mut read = json!({
        "spins": spins,
        "energy_milli": energy_milli(&spins, &p.h, &p.j, &p.edges),
        "timing": {"sample_us": 0, "total_us": 0},
        "sampler": sampler,
    });
    if let (true, Some(object)) = (embed, read.as_object_mut()) {
        let _ = object.insert(
            "embedding".into(),
            json!({"embedded": true, "chain_break_fraction": 0.0, "max_chain_length": CHAIN_LENGTH}),
        );
    }
    Ok(Value::Array(vec![read; p.num_reads]))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    if flag("--capabilities") {
        println!(
            "{}",
            json!({
                "backend": "dwave-qpu",
                "algorithm": "quantum-anneal",
                "maxNodes": 10_000,
                "maxEdges": 100_000,
                "features": ["anneal-time", "embed"],
            })
        );
        return ExitCode::SUCCESS;
    }
    if !flag("--solve") {
        eprintln!("usage: mock_qpu_solver --capabilities | --solve [--embed]");
        return ExitCode::from(64);
    }
    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() {
        return ExitCode::from(64);
    }
    match solve(&input, flag("--embed")) {
        Ok(reads) => {
            println!("{reads}");
            ExitCode::SUCCESS
        }
        Err(Refusal::Malformed(why)) => {
            eprintln!("malformed problem: {why}");
            ExitCode::from(64)
        }
        Err(Refusal::Capacity(why)) => {
            eprintln!("model exceeds this solver: {why}");
            ExitCode::from(65)
        }
    }
}
