//! A complete mock Quip solver in Rust, built against the published crates.
//!
//! A Rust solver supplies a `Sampler` and calls `run`. Everything else — the
//! four command-line modes, the handshake, credits, cancellation, and the exit
//! codes — belongs to `quip-solver-core`.
//!
//! The sampler is deliberately trivial: it returns `num_reads` copies of the
//! all-(+1) configuration, scored with the consensus scorer so the reported
//! energy is the one the network recomputes.

use clap::Parser;
use quip_solver_core::adapt::AdaptBounds;
// Reached through quip-solver-core rather than a second dependency.
use quip_solver_core::quip_protocol::scoring::energy_from_milli;
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, IsingGraph, SampleError, SampleParams, Sampler, SamplerResult,
};
use std::process::ExitCode;

/// Stands in for a device handle. A real backend owns its hardware here.
struct MockSampler;

impl Sampler for MockSampler {
    fn sample(
        &self,
        graph: &IsingGraph,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        // A real backend refuses a problem larger than its hardware.
        //
        // In session mode this branch never fires. The bound is the same
        // `max_nodes` advertised below, and the session rejects an oversized
        // job with `RejectReason::TooLarge` while parsing it, before the
        // sampler is called. `--solve` reads its problem straight from JSON
        // and applies no such bound, so that mode is what reaches this check.
        // A backend whose real capacity is smaller than what it advertises
        // would hit it from the session too.
        if graph.num_nodes() > 100_000 {
            return Err(SampleError::Capacity);
        }
        let spins = vec![1i8; graph.num_nodes()];
        // Score with the shipped scorer, never a local reimplementation: the
        // network recomputes this and rejects a solution that disagrees.
        let energy = energy_from_milli(&spins, &graph.h_milli, &graph.j_milli, &graph.edges);
        Ok((0..params.num_reads)
            .map(|_| SamplerResult {
                spins: spins.clone(),
                energy_milli: energy,
            })
            .collect())
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
            backend: "mock-rust",
            algorithm: "sa",
            max_nodes: 100_000,
            max_edges: 1_000_000,
            features: &[],
            adapt: AdaptBounds {
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
        || Ok(MockSampler),
    )
}
