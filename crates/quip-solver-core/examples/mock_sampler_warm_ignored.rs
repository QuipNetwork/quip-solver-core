//! Test-support miner that advertises warm starts but ignores the states: it
//! overrides `accepts_warm_start` and nothing else, so every read is all `+1`.
//! Used by `tests/loop_conformance.rs` to show the `initial-spins` grade fails
//! a solver that does not use the states it claims to use.

use clap::Parser;
use quip_protocol::scoring::energy_from_milli;
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, IsingGraph, SampleError, SampleParams, Sampler, SamplerResult,
};
use std::process::ExitCode;

struct IgnoringSampler;

impl Sampler for IgnoringSampler {
    fn sample(
        &self,
        graph: &IsingGraph,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        let spins = vec![1i8; graph.num_nodes()];
        let energy = energy_from_milli(&spins, &graph.h_milli, &graph.j_milli, &graph.edges);
        Ok((0..params.num_reads)
            .map(|_| SamplerResult {
                spins: spins.clone(),
                energy_milli: energy,
            })
            .collect())
    }

    fn accepts_warm_start() -> bool {
        true
    }
}

#[derive(Parser)]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), " protocol 1"))]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    run(
        BackendIdentity {
            backend: "mock",
            algorithm: "sa",
            max_nodes: 100_000,
            max_edges: 1_000_000,
            features: &[],
            adapt: quip_solver_core::adapt::AdaptBounds {
                min_sweeps: 64,
                max_sweeps: 4096,
                min_reads: 64,
                max_reads: 512,
                reads_solution_min_factor: 4,
                reads_solution_max_factor: 8,
                reads_solution_floor_factor: 0,
            },
        },
        &cli.common,
        || Ok(IgnoringSampler),
    )
}
