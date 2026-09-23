//! Test-support miner that uses warm starts: each seeded read returns its seed
//! unchanged, and every other read is all `+1`. Used by
//! `tests/loop_conformance.rs` to exercise the `initial-spins` conformance
//! grade.

use clap::Parser;
use quip_protocol::scoring::energy_milli;
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, IsingGraph, SampleError, SampleParams, Sampler,
    SamplerResult, WarmStart,
};
use std::process::ExitCode;

struct WarmSampler;

impl Sampler for WarmSampler {
    fn sample(
        &self,
        graph: &IsingGraph,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        let spins = vec![1i8; graph.num_nodes()];
        let energy = energy_milli(&spins, &graph.h, &graph.j, &graph.edges);
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

    fn sample_warm(
        &self,
        graph: &IsingGraph,
        params: &SampleParams,
        warm: &WarmStart,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        let mut reads = self.sample(graph, params)?;
        for (read, seed) in reads.iter_mut().zip(&warm.spins) {
            read.energy_milli = energy_milli(seed, &graph.h, &graph.j, &graph.edges);
            read.spins.clone_from(seed);
        }
        Ok(reads)
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
            backend: quip_proto::v1::Backend::Mock,
            algorithm: quip_proto::v1::Algorithm::Sa,
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
        || Ok(WarmSampler),
    )
}
