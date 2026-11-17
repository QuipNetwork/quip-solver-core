//! Test-support miner that lists the `calibrate` feature. `--calibrate` picks
//! how its hook answers: `done` returns a tuning, `fail` returns an error, and
//! `slow` returns a tuning only after the conformance walk's deadline has
//! passed. Used by `tests/loop_conformance.rs`.

use clap::{Parser, ValueEnum};
use quip_proto::v1::{Calibrate, Topology};
use quip_protocol::scoring::energy_milli;
use quip_solver_core::{
    run, BackendIdentity, Calibration, CommonArgs, IsingGraph, SampleError, SampleParams, Sampler,
    SamplerResult,
};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Longer than the conformance walk's two-second calibrate budget.
const SLOW_HOOK: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, ValueEnum)]
enum Mode {
    Done,
    Fail,
    Slow,
}

struct CalibratingSampler {
    mode: Mode,
    tuning: AtomicU32,
}

impl Sampler for CalibratingSampler {
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

    fn accepts_calibrate() -> bool {
        true
    }

    fn calibrate(
        &self,
        request: &Calibrate,
        topology: &Topology,
    ) -> Result<Calibration<Self>, SampleError> {
        if request.instances.is_empty() || topology.nodes.is_empty() {
            return Err(SampleError::Declined(
                "no reference instances or no topology".into(),
            ));
        }
        let next = self.tuning.load(Ordering::Relaxed) + 1;
        let tuned = Calibration::new(format!("mock tuning {next}"), move |s: &Self| {
            s.tuning.store(next, Ordering::Relaxed);
        });
        match self.mode {
            Mode::Done => Ok(tuned),
            Mode::Fail => Err(SampleError::Declined("mock calibration failed".into())),
            Mode::Slow => {
                std::thread::sleep(SLOW_HOOK);
                Ok(tuned)
            }
        }
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
    /// How the calibrate hook answers.
    #[arg(long, value_enum, default_value = "done")]
    calibrate: Mode,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mode = cli.calibrate;
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
            Ok(CalibratingSampler {
                mode,
                tuning: AtomicU32::new(1),
            })
        },
    )
}
