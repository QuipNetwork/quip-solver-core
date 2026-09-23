//! Width-two lease fixture. The coordinator controls every completion through a socket.
use clap::Parser;
use quip_solver_core::{
    coefficient::Milli, run, BackendIdentity, CancelToken, CommonArgs, IsingGraph, SampleError,
    SampleParams, Sampler, SamplerResult, StreamJob, StreamOutcome, StreamResult,
};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::ExitCode,
};
use tokio::sync::mpsc;

struct WideSampler {
    gate: PathBuf,
}
impl Sampler<Milli> for WideSampler {
    fn declared_stream_width() -> u32 {
        2
    }
    fn stream_width(&self) -> usize {
        2
    }
    fn sample(
        &self,
        graph: &IsingGraph<Milli>,
        _: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        let spins = vec![1; graph.num_nodes()];
        let h = graph.h.iter().map(|v| v.0).collect::<Vec<_>>();
        let j = graph.j.iter().map(|v| v.0).collect::<Vec<_>>();
        let energy_milli = quip_protocol::scoring::energy_from_milli(&spins, &h, &j, &graph.edges);
        Ok(vec![SamplerResult {
            spins,
            energy_milli,
        }])
    }
    fn sample_stream(
        &self,
        mut jobs: mpsc::Receiver<StreamJob<Milli>>,
        out: mpsc::Sender<StreamResult>,
        _: CancelToken,
    ) {
        std::thread::scope(|scope| {
            while let Some(job) = jobs.blocking_recv() {
                let out = out.clone();
                let _ = scope.spawn(move || {
                    let outcome = (|| {
                        let mut gate = std::os::unix::net::UnixStream::connect(&self.gate)?;
                        gate.write_all(
                            &u32::try_from(job.job_id.len())
                                .map_err(std::io::Error::other)?
                                .to_le_bytes(),
                        )?;
                        gate.write_all(&job.job_id)?;
                        gate.read_exact(&mut [0])?;
                        Ok::<_, std::io::Error>(())
                    })()
                    .map_err(|e| SampleError::DeviceFault(e.to_string()))
                    .and_then(|()| self.sample(&job.graph, &job.params));
                    let _ = out.blocking_send(StreamResult {
                        job_id: job.job_id,
                        outcome: StreamOutcome::Completed(outcome),
                        device_access_time_us: 0,
                    });
                });
            }
        });
    }
}
#[derive(Parser)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
    #[arg(long)]
    gate: PathBuf,
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
                min_sweeps: 1,
                max_sweeps: 4096,
                min_reads: 1,
                max_reads: 512,
                reads_solution_min_factor: 4,
                reads_solution_max_factor: 8,
                reads_solution_floor_factor: 0,
            },
        },
        &cli.common,
        || Ok(WideSampler { gate: cli.gate }),
    )
}
