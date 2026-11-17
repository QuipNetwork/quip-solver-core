//! Shared body of the three IQM mock solvers. Each solves only the `--solve`
//! IQM form (`"problem": "iqm"`) and refuses every other input with exit 64.
//! `mock_iqm` lists `IQM_SAMPLE` and reports the consensus energy of each
//! read. `mock_iqm_wrong_energy` lists it and reports one milli too much.
//! `mock_iqm_unlisted` solves the form without listing the kind.
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a --solve binary answers on stdout and explains refusals on stderr"
)]

use quip_protocol::scoring::iqm_energy_milli;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::io::Read as _;
use std::process::ExitCode;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IqmInput {
    problem: String,
    bounds: Vec<u32>,
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

fn solve(input: &[u8], energy_offset: i64) -> Result<Value, String> {
    let p: IqmInput = serde_json::from_slice(input).map_err(|e| e.to_string())?;
    if p.problem != "iqm"
        || p.bounds.len() != p.h.len()
        || p.j.len() != p.edges.len()
        || p.num_reads == 0
        || p.sweeps_per_beta == 0
        || p.options.is_some_and(|o| !o.is_empty())
    {
        return Err("malformed IQM problem".into());
    }
    let reads = (0..p.num_reads)
        .map(|read| {
            // Each read walks the values differently, all within the bounds.
            let values: Vec<u32> = p
                .bounds
                .iter()
                .enumerate()
                .map(|(i, &bound)| {
                    let step = u64::try_from(read + i).unwrap_or(0);
                    u32::try_from(step % (u64::from(bound) + 1)).unwrap_or(0)
                })
                .collect();
            let energy = iqm_energy_milli(&values, &p.h, &p.j, &p.edges) + energy_offset;
            json!({
                "values": values,
                "energy_milli": energy,
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
            })
        })
        .collect();
    Ok(Value::Array(reads))
}

/// Run the mock. `lists_iqm` is whether `--capabilities` lists `IQM_SAMPLE`.
/// `energy_offset` is added to every reported energy, 0 for a correct one.
pub(crate) fn run(lists_iqm: bool, energy_offset: i64) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--capabilities") {
        let kinds = if lists_iqm {
            json!(["ISING_SAMPLE", "IQM_SAMPLE"])
        } else {
            json!(["ISING_SAMPLE"])
        };
        println!(
            "{}",
            json!({
                "backend": "mock",
                "algorithm": "sa",
                "maxNodes": 10_000,
                "maxEdges": 100_000,
                "supportedKinds": kinds,
            })
        );
        return ExitCode::SUCCESS;
    }
    let mut input = Vec::new();
    if !args.iter().any(|a| a == "--solve") || std::io::stdin().read_to_end(&mut input).is_err() {
        return ExitCode::from(64);
    }
    match solve(&input, energy_offset) {
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
