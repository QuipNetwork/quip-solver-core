//! Test-support miner that hits a limit after it serves the lease. From then on
//! its `Status` reports `limit_reached`. It refuses jobs with `OVERLOADED`, as
//! SPEC section 4 requires, unless it runs with `--ignore-limit`, which keeps
//! serving them and so breaks the rule. Used by `tests/loop_conformance.rs`.
//!
//! The limit trips on observable protocol state: the fourth sample of the
//! lease graph, which is the last sampling the conformance walk asks for.

use clap::Parser;
use quip_protocol::scoring::energy_milli;
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, Headroom, IsingGraph, SampleError, SampleParams, Sampler,
    SamplerResult,
};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Nodes in the graph of the conformance lease. No other job of the walk has
/// as many.
const LEASE_NODES: usize = 8;

/// Salts in the conformance lease. The limit trips once all of them are done.
const LEASE_SALTS: usize = 4;

struct LimitedSampler {
    ignore_limit: bool,
    lease_samples: AtomicUsize,
}

impl LimitedSampler {
    fn limit_reached(&self) -> bool {
        self.lease_samples.load(Ordering::Relaxed) >= LEASE_SALTS
    }
}

impl Sampler for LimitedSampler {
    fn sample(
        &self,
        graph: &IsingGraph,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        if self.limit_reached() && !self.ignore_limit {
            return Err(SampleError::DeviceBusy);
        }
        let spins = vec![1i8; graph.num_nodes()];
        let energy = energy_milli(&spins, &graph.h, &graph.j, &graph.edges);
        let results = (0..params.num_reads)
            .map(|_| SamplerResult {
                spins: spins.clone(),
                energy_milli: energy,
            })
            .collect();
        if graph.num_nodes() == LEASE_NODES {
            let _ = self.lease_samples.fetch_add(1, Ordering::Relaxed);
        }
        Ok(results)
    }

    fn headroom(&self) -> Option<Headroom> {
        let limit_reached = self.limit_reached();
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
    /// Keep serving jobs after the limit trips, which SPEC section 4 forbids.
    #[arg(long)]
    ignore_limit: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ignore_limit = cli.ignore_limit;
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
        move || {
            Ok(LimitedSampler {
                ignore_limit,
                lease_samples: AtomicUsize::new(0),
            })
        },
    )
}
