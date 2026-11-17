//! Shared body of the two ladder mock solvers. Each lists `linear-schedule`,
//! returns all `+1` reads, and reports a `beta_ladder` of the kind it is given.
//! `mock_ladder_linear` runs the ladder it was asked for; `mock_ladder_geometric`
//! lists the feature but reports a geometric ladder; `mock_ladder_sdk` lists it
//! but reports the right ladder as the SDK default, with no override, as a kernel that ignores
//! the schedule would.
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
}

/// Rungs from `hot` to `cold`, evenly spaced or log-spaced.
fn ladder(kind: &str, (hot, cold): (f64, f64), rungs: usize) -> Vec<f64> {
    #[expect(clippy::cast_precision_loss, reason = "rung counts are small")]
    let t = |i: usize| i as f64 / (rungs.max(2) - 1) as f64;
    (0..rungs)
        .map(|i| match kind {
            "linear" => hot + t(i) * (cold - hot),
            _ => (hot.ln() + t(i) * (cold.ln() - hot.ln())).exp(),
        })
        .collect()
}

fn solve(input: &[u8], ran_kind: &str, reported_by: &str) -> Result<Value, String> {
    let p: Problem = serde_json::from_slice(input).map_err(|e| e.to_string())?;
    if p.j.len() != p.edges.len() || p.num_reads == 0 || p.sweeps_per_beta == 0 {
        return Err("malformed problem".into());
    }
    let asked = p.beta_schedule_type.as_deref().unwrap_or("geometric");
    let range = p.beta_range.unwrap_or((0.1, 1.0));
    let rungs = (p.num_sweeps / p.sweeps_per_beta).max(1);
    let spins = vec![1_i8; p.h.len()];
    let read = json!({
        "spins": spins,
        "energy_milli": energy_milli(&spins, &p.h, &p.j, &p.edges),
        "timing": {"sample_us": 0, "total_us": 0},
        "sampler": {
            "backend": "mock",
            "algorithm": "sa",
            "num_reads": p.num_reads,
            "num_sweeps": p.num_sweeps,
            "sweeps_per_beta": p.sweeps_per_beta,
            "beta_range": p.beta_range,
            // An echo of the request, so only the ladder tells what ran.
            "beta_schedule_type": asked,
            "beta_ladder": {
                "kind": ran_kind,
                "betas": ladder(ran_kind, range, rungs),
                "reported_by": reported_by,
            },
            "seed": p.seed,
        },
    });
    Ok(Value::Array(vec![read; p.num_reads]))
}

/// Run the mock. `ran_kind` is the ladder kind it reports, and `reported_by`
/// says who stated it, `"sampler"` or `"sdk"`.
pub(crate) fn run(ran_kind: &str, reported_by: &str) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--capabilities") {
        println!(
            "{}",
            json!({
                "backend": "mock",
                "algorithm": "sa",
                "maxNodes": 10_000,
                "maxEdges": 100_000,
                "features": ["linear-schedule"],
            })
        );
        return ExitCode::SUCCESS;
    }
    let mut input = Vec::new();
    if !args.iter().any(|a| a == "--solve") || std::io::stdin().read_to_end(&mut input).is_err() {
        return ExitCode::from(64);
    }
    match solve(&input, ran_kind, reported_by) {
        Ok(reads) => {
            println!("{reads}");
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("{why}");
            ExitCode::from(64)
        }
    }
}
