//! Test-support miner that advertises everything optional the `--solve` gate
//! grades: `linear-schedule`, `exact-coefficients`, and all three model
//! limits. It samples an int8 graph, so the SDK refuses anything off its
//! one-unit step. Used by `tests/solve_conformance.rs`.

use clap::Parser;
use quip_solver_core::coefficient::Fixed;
use quip_solver_core::quip_protocol::scoring::energy_milli;
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, IsingGraph, ModelLimits, SampleError, SampleParams, Sampler,
    SamplerResult, EXACT_COEFFICIENTS_FEATURE, LINEAR_SCHEDULE_FEATURE,
};
use std::process::ExitCode;

type Int8 = Fixed<i8, 1>;

struct LimitedSampler;

impl Sampler<Int8> for LimitedSampler {
    fn sample(
        &self,
        graph: &IsingGraph<Int8>,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        let units = |values: &[Int8]| values.iter().map(|v| f64::from(v.0)).collect::<Vec<_>>();
        let (h, j) = (units(&graph.h), units(&graph.j));
        // Spins depend on the seed and the read, so the same seed must give
        // the same samples and the gate can tell.
        Ok((0..params.num_reads)
            .map(|read| {
                let key = params.seed ^ u64::try_from(read).unwrap_or(u64::MAX);
                let spins: Vec<i8> = (0..graph.num_nodes())
                    .map(|i| if (key >> (i % 64)) & 1 == 0 { 1 } else { -1 })
                    .collect();
                let energy = energy_milli(&spins, &h, &j, &graph.edges);
                SamplerResult {
                    spins,
                    energy_milli: energy,
                }
            })
            .collect())
    }
}

#[derive(Parser)]
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
            max_nodes: 64,
            max_edges: 256,
            features: &[LINEAR_SCHEDULE_FEATURE, EXACT_COEFFICIENTS_FEATURE],
            adapt: quip_solver_core::adapt::AdaptBounds {
                min_sweeps: 64,
                max_sweeps: 4096,
                min_reads: 1,
                max_reads: 64,
                reads_solution_min_factor: 4,
                reads_solution_max_factor: 8,
                reads_solution_floor_factor: 0,
            },
            limits: ModelLimits {
                max_degree: Some(6),
                max_local_field_milli: Some(63_000),
                exact_resolution_milli: Some(1000),
            },
        },
        &cli.common,
        || Ok(LimitedSampler),
    )
}
