//! Test-support miner that reports `limit_reached` from its fourth `Status`
//! and then refuses jobs. It refuses them with `OVERLOADED`, as SPEC section 4 requires.
//! Used by `tests/loop_conformance.rs`.
//!
//! The conformance walk sends its fourth `Status` request, a `Ping`, after
//! every job it needs a `Result` for, so the limit trips only then.

use clap::Parser;
use quip_protocol::scoring::energy_milli;
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, Headroom, IsingGraph, SampleError, SampleParams, Sampler,
    SamplerResult,
};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The `Status` on which the limit trips.
const LIMIT_AT_STATUS: usize = 4;

#[derive(Default)]
struct LimitedSampler {
    statuses: AtomicUsize,
    limit_reached: AtomicBool,
}

impl Sampler for LimitedSampler {
    fn sample(
        &self,
        graph: &IsingGraph,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        if self.limit_reached.load(Ordering::Relaxed) {
            return Err(SampleError::DeviceBusy);
        }
        let spins = vec![1i8; graph.num_nodes()];
        let energy = energy_milli(&spins, &graph.h, &graph.j, &graph.edges);
        Ok((0..params.num_reads)
            .map(|_| SamplerResult {
                spins: spins.clone(),
                energy_milli: energy,
            })
            .collect())
    }

    fn headroom(&self) -> Option<Headroom> {
        let seen = self.statuses.fetch_add(1, Ordering::Relaxed) + 1;
        let limit_reached = seen >= LIMIT_AT_STATUS;
        self.limit_reached.store(limit_reached, Ordering::Relaxed);
        Some(Headroom {
            fraction: if limit_reached { 0.0 } else { 1.0 },
            limit_reached,
        })
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
            limits: quip_solver_core::ModelLimits::UNSTATED,
        },
        &cli.common,
        || Ok(LimitedSampler::default()),
    )
}
