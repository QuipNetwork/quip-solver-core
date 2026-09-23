//! Local lease test miner with honest, wrong-draw, and ignore-stop modes.

use clap::Parser;
use quip_protocol::scoring::energy_from_milli;
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, IsingGraph, Lease, LeaseSink, SampleError, SampleParams,
    Sampler, SamplerResult, TopologyView,
};
use std::process::ExitCode;

struct MockSampler {
    mode: String,
}

impl Sampler<quip_solver_core::coefficient::Milli> for MockSampler {
    fn generates_locally() -> bool {
        true
    }

    fn sample_lease(
        &self,
        lease: &Lease,
        topology: &TopologyView,
        _params: &SampleParams,
        out: &LeaseSink,
    ) -> Result<(), SampleError> {
        let mut logged = false;
        for i in 0..lease.salt_count() {
            if self.mode != "ignore-stop" && out.is_stopped() {
                break;
            }
            let spins = vec![1; topology.num_nodes];
            let (h, j) = topology
                .draw(lease.nonce(i))
                .map_err(|e| SampleError::DeviceFault(e.to_string()))?;
            let mut energy = energy_from_milli(&spins, &h, &j, &topology.edges);
            if self.mode == "wrong-draw" {
                // Select a different draw whose score is observably wrong.
                for n in 1u64.. {
                    let mut nonce = lease.nonce(i);
                    for (byte, other) in nonce.iter_mut().zip(n.to_le_bytes()) {
                        *byte ^= other;
                    }
                    let (h, j) = topology
                        .draw(nonce)
                        .map_err(|e| SampleError::DeviceFault(e.to_string()))?;
                    let wrong = energy_from_milli(&spins, &h, &j, &topology.edges);
                    if wrong != energy {
                        energy = wrong;
                        break;
                    }
                }
            }
            if out
                .push(
                    i,
                    vec![SamplerResult {
                        spins,
                        energy_milli: energy,
                    }],
                )
                .is_err()
            {
                if self.mode != "ignore-stop" {
                    break;
                }
                if !logged {
                    tracing::error!("push returned LeaseStopped");
                    logged = true;
                }
            }
            if self.mode == "ignore-stop" {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        Ok(())
    }

    fn sample(
        &self,
        graph: &IsingGraph<quip_solver_core::coefficient::Milli>,
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
    #[arg(long, default_value = "honest")]
    mode: String,
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
        || Ok(MockSampler { mode: cli.mode }),
    )
}
