//! Test-support miner whose `open()` always fails. Used by
//! `tests/driver_mode.rs` to exercise the `EnvIncompatible` (exit 69) path.

use clap::Parser;
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, IsingGraph, OpenError, SampleError, SampleParams, Sampler,
    SamplerResult,
};
use std::process::ExitCode;

struct UnopenableSampler;

impl Sampler for UnopenableSampler {
    fn sample(
        &self,
        _graph: &IsingGraph,
        _params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        // Unreachable: `open` fails before the sampler is used.
        Ok(Vec::new())
    }
}

static CLI_VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "{} protocol {}",
        env!("CARGO_PKG_VERSION"),
        quip_protocol::session::PROTOCOL_VERSION
    )
});

#[derive(Parser)]
#[command(version = CLI_VERSION.as_str())]
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
        || -> Result<UnopenableSampler, OpenError> {
            Err(OpenError("test: device not present".to_owned()))
        },
    )
}
