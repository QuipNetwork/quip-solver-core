//! Test-support miner: runs the core loop with a trivial sampler that returns
//! `num_reads` all-`+1` solutions, scored with the consensus scorer. Used by
//! `tests/lease_session.rs` to exercise the session loop without a backend.

use clap::Parser;
use quip_protocol::scoring::energy_from_milli;
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, IsingGraph, NativeTopology, SampleError, SampleParams,
    Sampler, SamplerResult,
};
use std::process::ExitCode;

struct MockSampler {
    zero: bool,
    gate: Option<std::path::PathBuf>,
    native: Option<NativeTopology>,
}

impl Sampler<quip_solver_core::coefficient::Milli> for MockSampler {
    fn native_topology(&self) -> Option<NativeTopology> {
        self.native.clone()
    }

    fn sample(
        &self,
        graph: &IsingGraph<quip_solver_core::coefficient::Milli>,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        if let Some(path) = &self.gate {
            use std::io::Read as _;
            let mut stream = std::os::unix::net::UnixStream::connect(path)
                .map_err(|e| SampleError::DeviceFault(e.to_string()))?;
            stream
                .read_exact(&mut [0])
                .map_err(|e| SampleError::DeviceFault(e.to_string()))?;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
        if self.zero {
            return Ok(vec![]);
        }
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
    #[arg(long)]
    zero: bool,
    #[arg(long)]
    gate: Option<std::path::PathBuf>,
    /// The working graph the mock device reports: `hosts` is the test
    /// topology (qubits 10 and 20, coupler 10-20), `lacks` has lost qubit 20.
    #[arg(long)]
    native: Option<String>,
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
        || {
            let native = match cli.native.as_deref() {
                Some("hosts") => Some(NativeTopology::new(vec![10, 20], vec![(10, 20)])),
                Some("lacks") => Some(NativeTopology::new(vec![10, 30], vec![(10, 30)])),
                Some(other) => {
                    return Err(quip_solver_core::OpenError(format!(
                        "unknown native graph {other}"
                    )))
                }
                None => None,
            };
            Ok(MockSampler {
                zero: cli.zero,
                gate: cli.gate,
                native,
            })
        },
    )
}
