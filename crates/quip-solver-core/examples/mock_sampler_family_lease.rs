//! Test-support miner that generates family leases: the trivial sampler of
//! `mock_sampler_lease` (all-`+1` spins scored with the consensus scorer),
//! with `solver_id` `quip-mock/family-lease` and the layered test
//! construction on `vectors/uniform-field` and `vectors/restricted-field`.
//! Used by `tests/family_session.rs`.

use clap::Parser;
use quip_protocol::scoring::energy_from_milli;
use quip_solver_core::coefficient::Milli;
use quip_solver_core::families::{Declaration, FamilyClaim};
use quip_solver_core::ising_generators::descriptor::Contract;
use quip_solver_core::ising_generators::instance::InstanceLimits;
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, IsingGraph, ModelLimits, SampleError, SampleParams, Sampler,
    SamplerResult,
};
use std::process::ExitCode;

#[path = "support/layered_family.rs"]
mod layered;

use layered::{example_size_bounds, UNIFORM_FIELD};

/// `P_SPEC`: two layers, field -1500.
const P_SPEC: [u8; 19] = [
    0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0x24, 0xfa, 0xff, 0xff,
];

const TIGHT_LIMITS: InstanceLimits = match InstanceLimits::new(
    16_777_216,
    67_108_864,
    2000,
    72_057_594_037_927_936,
    67_108_864,
) {
    Some(limits) => limits,
    None => InstanceLimits::FAM_37,
};

const UNIFORM_V1: Contract = Contract {
    family_id: "vectors/uniform-field",
    revision: 1,
    schema: UNIFORM_FIELD,
    metadata_kinds: &[1],
    limits: TIGHT_LIMITS,
    size_bounds: example_size_bounds,
    construction: Some(&layered::LAYERED),
};

const RESTRICTED_V1: Contract = Contract {
    family_id: "vectors/restricted-field",
    ..UNIFORM_V1
};

const CLAIMS: &[FamilyClaim] = &[
    FamilyClaim {
        revisions: &[UNIFORM_V1],
        only_params: &[],
    },
    FamilyClaim {
        revisions: &[RESTRICTED_V1],
        only_params: &[&P_SPEC],
    },
];

/// Samples the graph the session hands over: the regenerated instance for a
/// family lease, the legacy draw otherwise.
struct DefaultFamily {
    gate: Option<std::path::PathBuf>,
}

impl Sampler<Milli> for DefaultFamily {
    fn declaration() -> Declaration {
        Declaration {
            solver_id: "quip-mock/family-lease",
            generated: CLAIMS,
            accepted: CLAIMS,
        }
    }

    fn sample(
        &self,
        graph: &IsingGraph<Milli>,
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

/// Which lease path the binary exercises.
#[derive(Clone, Copy, clap::ValueEnum)]
enum Mode {
    /// The session expands each salt and streams the instance to `sample`.
    Default,
}

#[derive(Parser)]
#[command(version = concat!(env!("CARGO_PKG_VERSION"), " protocol 2"))]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
    #[arg(long, value_enum, default_value_t = Mode::Default)]
    mode: Mode,
    #[arg(long)]
    gate: Option<std::path::PathBuf>,
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
        limits: ModelLimits {
            max_degree: Some(4),
            ..ModelLimits::UNSTATED
        },
    };
    match cli.mode {
        Mode::Default => run(id, &cli.common, || Ok(DefaultFamily { gate: cli.gate })),
    }
}
