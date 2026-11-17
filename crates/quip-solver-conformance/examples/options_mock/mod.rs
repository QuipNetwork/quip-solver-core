//! Shared body of the two option mock solvers. Each advertises the `solveOptions`
//! `restarts` (integer) and `greedy` (boolean) and returns all `+1` reads.
//! `mock_options_strict` refuses an undeclared name or a wrong-type value with
//! exit 64, as the SDK does. `mock_options_lax` accepts any `options` object,
//! as a solver that skips the check would.
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a --solve binary answers on stdout and explains refusals on stderr"
)]

use quip_protocol::scoring::energy_milli;
use serde::Deserialize;
use serde_json::{json, Map, Value};
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
    seed: u64,
    options: Option<Map<String, Value>>,
}

/// Refuse a name the mock does not list, or a value of the wrong type.
fn check_options(options: &Map<String, Value>) -> Result<(), String> {
    for (name, value) in options {
        let ok = match name.as_str() {
            "restarts" => value.is_i64(),
            "greedy" => value.is_boolean(),
            _ => return Err(format!("option {name} is not accepted")),
        };
        if !ok {
            return Err(format!("option {name} has the wrong type"));
        }
    }
    Ok(())
}

fn solve(input: &[u8], strict: bool) -> Result<Value, String> {
    let p: Problem = serde_json::from_slice(input).map_err(|e| e.to_string())?;
    if p.j.len() != p.edges.len() || p.num_reads == 0 || p.sweeps_per_beta == 0 {
        return Err("malformed problem".into());
    }
    if let (true, Some(options)) = (strict, &p.options) {
        check_options(options)?;
    }
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
            "beta_schedule_type": "geometric",
            "seed": p.seed,
        },
    });
    Ok(Value::Array(vec![read; p.num_reads]))
}

/// Run the mock. `strict` is whether it validates `options`.
pub(crate) fn run(strict: bool) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--capabilities") {
        println!(
            "{}",
            json!({
                "backend": "mock",
                "algorithm": "sa",
                "maxNodes": 10_000,
                "maxEdges": 100_000,
                "solveOptions": [
                    {"name": "restarts", "type": "OPTION_TYPE_INT"},
                    {"name": "greedy", "type": "OPTION_TYPE_BOOL"},
                ],
            })
        );
        return ExitCode::SUCCESS;
    }
    let mut input = Vec::new();
    if !args.iter().any(|a| a == "--solve") || std::io::stdin().read_to_end(&mut input).is_err() {
        return ExitCode::from(64);
    }
    match solve(&input, strict) {
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
