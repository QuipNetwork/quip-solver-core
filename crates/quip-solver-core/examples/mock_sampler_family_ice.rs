//! Test-support miner that lists two ice families as generated and accepted:
//! the trivial sampler of `mock_sampler_lease` (all-`+1` spins scored with the
//! consensus scorer), with `solver_id` `quip-mock/family-ice`. Used by
//! `tests/family_drive.rs`.

use clap::Parser;
use quip_protocol::scoring::energy_from_milli;
use quip_solver_core::coefficient::Milli;
use quip_solver_core::families::{Declaration, FamilyClaim};
use quip_solver_core::ising_generators::ice::{ICE_GLASS_V1, TWISTED_DIMER_V1};
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, IsingGraph, ModelLimits, SampleError, SampleParams, Sampler,
    SamplerResult,
};
use std::process::ExitCode;

const CLAIMS: &[FamilyClaim] = &[
    FamilyClaim {
        revisions: &[ICE_GLASS_V1],
        only_params: &[],
    },
    FamilyClaim {
        revisions: &[TWISTED_DIMER_V1],
        only_params: &[],
    },
];

/// Solves the graph the session hands over: the regenerated instance of a
/// lease salt or the plain model of an explicit job.
struct IceSampler;

impl Sampler<Milli> for IceSampler {
    fn declaration() -> Declaration {
        Declaration {
            solver_id: "quip-mock/family-ice",
            generated: CLAIMS,
            accepted: CLAIMS,
        }
    }

    fn sample(
        &self,
        graph: &IsingGraph<Milli>,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        let spins = vec![1i8; graph.num_nodes()];
        let h = graph.h.iter().map(|v| v.0).collect::<Vec<_>>();
        let j = graph.j.iter().map(|v| v.0).collect::<Vec<_>>();
        let energy = energy_from_milli(&spins, &h, &j, &graph.edges);
        Ok((0..params.num_reads)
            .map(|_| SamplerResult {
                spins: spins.clone(),
                energy_milli: energy,
            })
            .collect())
    }
}

#[derive(Parser)]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), " protocol 2"))]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let id = BackendIdentity {
        backend: quip_proto::v1::Backend::Mock,
        algorithm: quip_proto::v1::Algorithm::Sa,
        max_nodes: 0,
        max_edges: 0,
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
        limits: ModelLimits::UNSTATED,
    };
    run(id, &cli.common, || Ok(IceSampler))
}
